//! Local analysis of immutable archived PDFs. This module never touches posting queues.
use anyhow::{bail, ensure, Context, Result};
use chrono::Utc;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, JsonSpec, ReasoningEffort, StopReason};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

use crate::{db::Database, summarizer::MODEL};

// Bump when changing the prompt/schema or extraction/validation behaviour.
const PROMPT_VERSION: i64 = 2;
const MAX_POST_CHARS: usize = 300;
const EXTRACTOR_VERSION: &str = "pdf-extract-0.12.1-v1";
const MAX_ATTEMPTS: i64 = 3;
const MAX_TEXT_BYTES: usize = 120_000;
const MAX_PAGES: usize = 100;
const PROMPT: &str = include_str!("conditions_prompt.txt");

pub fn initialize_schema(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS ConditionsSummaries (
            DocumentVersionId INTEGER NOT NULL REFERENCES ApprovalDocumentVersions(Id),
            Model TEXT NOT NULL,
            PromptVersion INTEGER NOT NULL,
            ExtractorVersion TEXT NOT NULL,
            PagesJson TEXT,
            SummaryJson TEXT,
            RawResponse TEXT,
            Attempts INTEGER NOT NULL DEFAULT 0,
            LastAttempt INTEGER,
            CompletedAt INTEGER,
            LastError TEXT,
            PRIMARY KEY(DocumentVersionId, Model, PromptVersion, ExtractorVersion)
        );",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub requirement: String,
    pub page: usize,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConditionsSummary {
    pub overview: String,
    pub requirements: Vec<Requirement>,
    pub limitations: Vec<String>,
}

#[derive(Debug)]
pub struct SummaryRecord {
    pub document_version_id: i64,
    pub project_name: String,
    pub source_url: String,
    pub summary: ConditionsSummary,
}

impl SummaryRecord {
    pub fn post_text(&self) -> Result<String> {
        format_post(&self.summary, &self.source_url).with_context(|| {
            format!(
                "Cannot format conditions post for {} (PDF version {})",
                self.project_name, self.document_version_id
            )
        })
    }
}

fn overview_budget(source_url: &str) -> usize {
    MAX_POST_CHARS.saturating_sub(source_url.chars().count() + 1)
}

fn format_post(summary: &ConditionsSummary, source_url: &str) -> Result<String> {
    // Counting Unicode scalar values is conservative: never more permissive than
    // Bluesky's 300-grapheme limit. Include the complete URL and separator.
    ensure!(
        summary.overview.chars().count() <= overview_budget(source_url),
        "Conditions post exceeds its character budget including the source URL"
    );
    Ok(format!("{}\n{}", summary.overview, source_url))
}

#[derive(Default, Debug)]
pub struct SummaryRun {
    pub summaries: Vec<SummaryRecord>,
    pub failures: Vec<String>,
}

struct SummaryInput {
    project_name: String,
    source_url: String,
    pages: Vec<String>,
}

fn response_schema() -> serde_json::Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            // Validate length after generation. Constrained decoding with maxLength
            // can force a string to end mid-sentence instead of rewriting it.
            "overview": {"type": "string"},
            "requirements": {"type": "array", "maxItems": 12, "items": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "requirement": {"type": "string", "maxLength": 1500},
                    "page": {"type": "integer", "minimum": 1, "maximum": 100},
                    "evidence": {"type": "string", "maxLength": 1500}
                }, "required": ["requirement", "page", "evidence"]
            }},
            "limitations": {"type": "array", "maxItems": 8, "items": {"type": "string", "maxLength": 1000}}
        }, "required": ["overview", "requirements", "limitations"]
    })
}

fn request(input: &SummaryInput) -> ChatRequest {
    let pages: Vec<_> = input
        .pages
        .iter()
        .enumerate()
        .map(|(index, text)| json!({"page": index + 1, "text": text}))
        .collect();
    ChatRequest::new(vec![ChatMessage::user(
        json!({
            "project_name": input.project_name,
            "source_url": input.source_url,
            "overview_character_limit": overview_budget(&input.source_url),
            "pages": pages,
        })
        .to_string(),
    )])
    .with_system(PROMPT)
}

trait SummaryModel {
    async fn summarize(&self, input: &SummaryInput) -> Result<String>;
}

struct OpenAiSummarizer(genai::Client);

impl SummaryModel for OpenAiSummarizer {
    async fn summarize(&self, input: &SummaryInput) -> Result<String> {
        let options = ChatOptions::default()
            .with_reasoning_effort(ReasoningEffort::Low)
            .with_max_tokens(5000)
            .with_response_format(JsonSpec::new("conditions_summary", response_schema()));
        let response = tokio::time::timeout(
            Duration::from_secs(90),
            self.0.exec_chat(MODEL, request(input), Some(&options)),
        )
        .await
        .context("Conditions summary request timed out")??;
        ensure!(
            matches!(response.stop_reason, Some(StopReason::Completed(_))),
            "Conditions summary was not completed: {:?}",
            response.stop_reason
        );
        Ok(response
            .first_text()
            .context("Model returned no summary text")?
            .to_string())
    }
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn validate_pages(pages: &[String]) -> Result<()> {
    ensure!(
        !pages.is_empty() && pages.len() <= MAX_PAGES,
        "PDF must contain 1–{MAX_PAGES} pages; no text was sent to the model"
    );
    ensure!(
        pages.iter().map(String::len).sum::<usize>() <= MAX_TEXT_BYTES,
        "Extracted PDF exceeds {MAX_TEXT_BYTES} bytes; refusing to truncate conditions"
    );
    for (index, page) in pages.iter().enumerate() {
        ensure!(
            page.chars().filter(|c| c.is_alphanumeric()).count() >= 40,
            "PDF page {} has too little readable text; may need OCR or manual review",
            index + 1
        );
    }
    Ok(())
}

async fn extract_pages(bytes: Vec<u8>) -> Result<Vec<String>> {
    ensure!(
        bytes.len() <= 20 * 1024 * 1024,
        "PDF exceeds 20 MiB extraction limit"
    );
    let pages =
        tokio::task::spawn_blocking(move || pdf_extract::extract_text_from_mem_by_pages(&bytes))
            .await
            .context("PDF extractor failed")?
            .context("Cannot extract PDF text")?;
    validate_pages(&pages)?;
    Ok(pages)
}

fn parse_summary(text: &str, pages: &[String]) -> Result<ConditionsSummary> {
    let summary: ConditionsSummary =
        serde_json::from_str(text).context("Invalid conditions summary JSON")?;
    ensure!(
        !summary.overview.trim().is_empty()
            && summary.overview.chars().count() <= MAX_POST_CHARS
            && !summary.overview.chars().any(char::is_control),
        "Summary overview is empty or too long"
    );
    ensure!(
        summary.overview.ends_with('.'),
        "Conditions post must end with a complete sentence"
    );
    ensure!(
        summary.requirements.len() <= 12,
        "Summary has more than 12 requirements"
    );
    ensure!(
        !summary.requirements.is_empty() || !summary.limitations.is_empty(),
        "Empty summary must explain its limitations"
    );
    ensure!(
        summary.limitations.len() <= 8
            && summary
                .limitations
                .iter()
                .all(|s| !s.trim().is_empty() && s.len() <= 1000),
        "Invalid summary limitations"
    );
    for item in &summary.requirements {
        ensure!(
            !item.requirement.trim().is_empty() && item.requirement.len() <= 1500,
            "Requirement is empty or too long"
        );
        let page = item
            .page
            .checked_sub(1)
            .and_then(|i| pages.get(i))
            .context("Summary cites a nonexistent PDF page")?;
        let quote = normalized(&item.evidence);
        ensure!(
            (20..=1500).contains(&quote.len()),
            "Evidence excerpt is too short or too long"
        );
        ensure!(
            normalized(page).contains(&quote),
            "Evidence does not match PDF page {}",
            item.page
        );
    }
    Ok(summary)
}

fn selected_versions(
    db: &Connection,
    limit: usize,
    version: Option<i64>,
    retry_failed: bool,
) -> Result<Vec<i64>> {
    ensure!(
        (1..=100).contains(&limit),
        "Summary limit must be between 1 and 100"
    );
    if let Some(id) = version {
        ensure!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM ApprovalDocumentVersions WHERE Id = ?1)",
                [id],
                |r| r.get::<_, bool>(0)
            )?,
            "Archived PDF version {id} does not exist"
        );
    }
    let mut stmt = db.prepare(
        "SELECT v.Id FROM ApprovalDocumentVersions v
         LEFT JOIN ConditionsSummaries s ON s.DocumentVersionId = v.Id
           AND s.Model = ?1 AND s.PromptVersion = ?2 AND s.ExtractorVersion = ?3
         WHERE (?4 IS NULL OR v.Id = ?4)
           AND (?4 IS NOT NULL OR s.SummaryJson IS NULL)
           AND (s.SummaryJson IS NOT NULL OR ?5 OR coalesce(s.Attempts, 0) < ?6)
         ORDER BY v.Id DESC LIMIT ?7",
    )?;
    let rows = stmt.query_map(
        params![
            MODEL,
            PROMPT_VERSION,
            EXTRACTOR_VERSION,
            version,
            retry_failed,
            MAX_ATTEMPTS,
            limit as i64
        ],
        |r| r.get(0),
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub async fn summarize_conditions(
    db: &Database,
    limit: usize,
    version: Option<i64>,
    retry_failed: bool,
) -> Result<SummaryRun> {
    let ids = selected_versions(db, limit, version, retry_failed)?;
    // Fail before consuming per-document attempts if the whole run lacks credentials.
    for id in &ids {
        if cached_summary(db, *id)?.is_none() {
            ensure!(
                std::env::var("OPENAI_API_KEY").is_ok_and(|key| !key.trim().is_empty()),
                "OPENAI_API_KEY is required to summarize conditions"
            );
            break;
        }
    }
    summarize_versions(db, &ids, &OpenAiSummarizer(genai::Client::default())).await
}

fn cached_summary(db: &Connection, id: i64) -> Result<Option<String>> {
    Ok(db
        .query_row(
            "SELECT SummaryJson FROM ConditionsSummaries WHERE DocumentVersionId = ?1
         AND Model = ?2 AND PromptVersion = ?3 AND ExtractorVersion = ?4",
            params![id, MODEL, PROMPT_VERSION, EXTRACTOR_VERSION],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

async fn summarize_versions(
    db: &Database,
    ids: &[i64],
    model: &impl SummaryModel,
) -> Result<SummaryRun> {
    let mut run = SummaryRun::default();
    for &id in ids {
        let (project_name, source_url): (String, String) = db.query_row(
            "SELECT coalesce((SELECT ProjectName FROM ApprovalEvents e WHERE e.ProjectId = d.ProjectId ORDER BY Id DESC LIMIT 1), d.Title), d.SourceUrl
             FROM ApprovalDocumentVersions v JOIN ApprovalDocuments d ON d.Id = v.DocumentId WHERE v.Id = ?1",
            [id], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if let Some(cached) = cached_summary(db, id)? {
            run.summaries.push(SummaryRecord {
                document_version_id: id,
                project_name,
                source_url,
                summary: serde_json::from_str(&cached)?,
            });
            continue;
        }
        let now = Utc::now().timestamp();
        db.execute(
            "INSERT INTO ConditionsSummaries(DocumentVersionId, Model, PromptVersion, ExtractorVersion, Attempts, LastAttempt)
             VALUES (?1, ?2, ?3, ?4, 1, ?5)
             ON CONFLICT(DocumentVersionId, Model, PromptVersion, ExtractorVersion)
             DO UPDATE SET Attempts = Attempts + 1, LastAttempt = excluded.LastAttempt, RawResponse = NULL",
            params![id, MODEL, PROMPT_VERSION, EXTRACTOR_VERSION, now],
        )?;
        let result = summarize_one(db, id, &project_name, &source_url, model).await;
        match result {
            Ok(summary) => {
                db.execute("UPDATE ConditionsSummaries SET SummaryJson = ?5, CompletedAt = ?6, LastError = NULL
                    WHERE DocumentVersionId = ?1 AND Model = ?2 AND PromptVersion = ?3 AND ExtractorVersion = ?4",
                    params![id, MODEL, PROMPT_VERSION, EXTRACTOR_VERSION, serde_json::to_string(&summary)?, Utc::now().timestamp()])?;
                run.summaries.push(SummaryRecord {
                    document_version_id: id,
                    project_name,
                    source_url,
                    summary,
                });
            }
            Err(error) => {
                let message = format!("{error:#}");
                db.execute("UPDATE ConditionsSummaries SET LastError = ?5
                    WHERE DocumentVersionId = ?1 AND Model = ?2 AND PromptVersion = ?3 AND ExtractorVersion = ?4",
                    params![id, MODEL, PROMPT_VERSION, EXTRACTOR_VERSION, message])?;
                run.failures
                    .push(format!("PDF version {id} ({project_name}): {message}"));
            }
        }
    }
    Ok(run)
}

async fn summarize_one(
    db: &Database,
    id: i64,
    project_name: &str,
    source_url: &str,
    model: &impl SummaryModel,
) -> Result<ConditionsSummary> {
    ensure!(
        overview_budget(source_url) >= 40,
        "Source URL leaves too little room for a conditions post"
    );
    let saved: Option<String> = db.query_row("SELECT PagesJson FROM ConditionsSummaries
        WHERE DocumentVersionId = ?1 AND Model = ?2 AND PromptVersion = ?3 AND ExtractorVersion = ?4",
        params![id, MODEL, PROMPT_VERSION, EXTRACTOR_VERSION], |r| r.get(0))?;
    let pages = if let Some(saved) = saved {
        let pages: Vec<String> = serde_json::from_str(&saved)?;
        validate_pages(&pages)?;
        pages
    } else {
        let bytes: Vec<u8> = db.query_row(
            "SELECT Content FROM ApprovalDocumentVersions WHERE Id = ?1",
            [id],
            |r| r.get(0),
        )?;
        let pages = extract_pages(bytes).await?;
        db.execute("UPDATE ConditionsSummaries SET PagesJson = ?5
            WHERE DocumentVersionId = ?1 AND Model = ?2 AND PromptVersion = ?3 AND ExtractorVersion = ?4",
            params![id, MODEL, PROMPT_VERSION, EXTRACTOR_VERSION, serde_json::to_string(&pages)?])?;
        pages
    };
    let input = SummaryInput {
        project_name: project_name.into(),
        source_url: source_url.into(),
        pages,
    };
    let response = model.summarize(&input).await?;
    // Keep the model's last response for review even when citation validation fails.
    db.execute("UPDATE ConditionsSummaries SET RawResponse = ?5
        WHERE DocumentVersionId = ?1 AND Model = ?2 AND PromptVersion = ?3 AND ExtractorVersion = ?4",
        params![id, MODEL, PROMPT_VERSION, EXTRACTOR_VERSION, response])?;
    let summary = parse_summary(&response, &input.pages)?;
    format_post(&summary, source_url)?;
    Ok(summary)
}

pub async fn run_command(
    database: &str,
    limit: usize,
    version: Option<i64>,
    retry_failed: bool,
) -> Result<()> {
    let db = Database::new_from_file(database)?;
    let run = summarize_conditions(&db, limit, version, retry_failed).await?;
    for record in &run.summaries {
        println!("{}\n", record.post_text()?);
    }
    if !run.failures.is_empty() {
        bail!(
            "Conditions summarization failed for {} document(s): {}",
            run.failures.len(),
            run.failures.join("; ")
        );
    }
    if run.summaries.is_empty() {
        println!("No eligible archived PDFs. Completed summaries are cached; failures stop retrying after {MAX_ATTEMPTS} attempts. Use --document-version to view a cached summary or --retry-failed-summaries to retry exhausted failures.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const PDF: &[u8] = include_bytes!("../test_files/conditions-renfrew.pdf");

    fn pages() -> Vec<String> {
        vec!["Provide the required five (5) Class B bicycle spaces in accordance with the minimum requirements.".into()]
    }

    fn answer(pages: &[String]) -> String {
        json!({
            "overview": "The letter requires changes before a permit can be issued.",
            "requirements": [{"requirement": "Provide five Class B bicycle spaces.", "page": 1,
                "evidence": normalized(&pages[0]).chars().take(100).collect::<String>()}],
            "limitations": []
        })
        .to_string()
    }

    struct FakeModel {
        calls: Cell<usize>,
        fail: Cell<bool>,
    }
    impl FakeModel {
        fn new(fail: bool) -> Self {
            Self {
                calls: Cell::new(0),
                fail: Cell::new(fail),
            }
        }
    }
    impl SummaryModel for FakeModel {
        async fn summarize(&self, input: &SummaryInput) -> Result<String> {
            self.calls.set(self.calls.get() + 1);
            if self.fail.get() {
                bail!("simulated model failure");
            }
            Ok(answer(&input.pages))
        }
    }

    fn seed(db: &Database, id: i64, bytes: &[u8]) {
        db.execute("INSERT OR IGNORE INTO ApprovalDocuments(Id, ProjectId, SourceUrl, Title, FirstSeen, LastSeen)
            VALUES (1, '52161', 'https://www.shapeyourcity.ca/52161/widgets/220724/documents/172212', 'Renfrew conditions', 1, 1)", []).unwrap();
        db.execute("INSERT INTO ApprovalDocumentVersions(Id, DocumentId, DownloadUrl, FirstSeen, LastSeen, Content)
            VALUES (?1, 1, 'https://example.com/letter.pdf', 1, 1, ?2)", params![id, bytes]).unwrap();
    }

    #[tokio::test]
    async fn extracts_all_six_pages_of_real_conditions_letter() {
        let extracted = extract_pages(PDF.to_vec()).await.unwrap();
        assert_eq!(extracted.len(), 6);
        assert!(normalized(&extracted[0]).contains("DP-2026-00087"));
        assert!(normalized(&extracted[1]).contains("five (5) Class B bicycle spaces"));
        assert!(normalized(&extracted[3]).contains("October 16, 2026"));
    }

    #[test]
    fn refuses_scans_partial_text_and_oversized_input_instead_of_truncating() {
        for input in [
            vec![],
            vec!["".into()],
            vec![pages()[0].clone(), "Page 2".into()],
            vec!["A".repeat(MAX_TEXT_BYTES + 1)],
            vec![pages()[0].clone(); MAX_PAGES + 1],
        ] {
            assert!(validate_pages(&input).is_err());
        }
        assert!(validate_pages(&pages()).is_ok());
    }

    #[tokio::test]
    async fn malformed_pdf_is_a_recoverable_error() {
        assert!(extract_pages(b"not a pdf".to_vec()).await.is_err());
    }

    #[test]
    fn post_budget_includes_full_url_and_rejects_overflow_without_truncating() {
        let mut summary = parse_summary(&answer(&pages()), &pages()).unwrap();
        let url = format!("https://example.com/{}", "a".repeat(100));
        let budget = overview_budget(&url);
        summary.overview = "é".repeat(budget);
        let post = format_post(&summary, &url).unwrap();
        assert_eq!(post.chars().count(), 300);
        assert!(post.ends_with(&url));
        summary.overview.push('!');
        assert!(format_post(&summary, &url).is_err());
        assert_eq!(overview_budget("https://example.com"), 280);
        assert_eq!(overview_budget(&"a".repeat(400)), 0);
        // A post may use spare room after the link; 200 is not a separate cap.
        summary.overview = format!("{}.", "a".repeat(220));
        assert!(format_post(&summary, "https://example.com").is_ok());
        assert!(parse_summary(&serde_json::to_string(&summary).unwrap(), &pages()).is_ok());
    }

    #[test]
    fn rejects_long_or_multiline_post_text() {
        let mut value: serde_json::Value = serde_json::from_str(&answer(&pages())).unwrap();
        for overview in [
            "a".repeat(MAX_POST_CHARS + 1),
            "First paragraph.\nSecond paragraph.".into(),
            "Requires either four drop".into(),
            "Loading outside operating\u{2}?".into(),
        ] {
            value["overview"] = overview.into();
            assert!(parse_summary(&value.to_string(), &pages()).is_err());
        }
    }

    #[test]
    fn validates_quotes_with_different_whitespace() {
        let mut summary: serde_json::Value = serde_json::from_str(&answer(&pages())).unwrap();
        summary["requirements"][0]["evidence"] =
            "Provide the required\nfive (5) Class B bicycle spaces".into();
        assert!(parse_summary(&summary.to_string(), &pages()).is_ok());
    }

    #[test]
    fn rejects_invalid_pages_fabricated_quotes_and_malformed_summaries() {
        let base: serde_json::Value = serde_json::from_str(&answer(&pages())).unwrap();
        for page in [0, 2, 999] {
            let mut value = base.clone();
            value["requirements"][0]["page"] = page.into();
            assert!(parse_summary(&value.to_string(), &pages()).is_err());
        }
        for evidence in ["", "five", "The developer must pay a million dollars."] {
            let mut value = base.clone();
            value["requirements"][0]["evidence"] = evidence.into();
            assert!(parse_summary(&value.to_string(), &pages()).is_err());
        }
        for response in ["", "not JSON", "{}", "{\"overview\":\"unfinished"] {
            assert!(parse_summary(response, &pages()).is_err());
        }
        let mut unknown = base.clone();
        unknown["estimated_cost"] = "invented".into();
        assert!(parse_summary(&unknown.to_string(), &pages()).is_err());
        let mut empty = base;
        empty["requirements"] = json!([]);
        assert!(parse_summary(&empty.to_string(), &pages()).is_err());
        empty["limitations"] = json!(["This is not a conditions letter."]);
        assert!(parse_summary(&empty.to_string(), &pages()).is_ok());
    }

    #[tokio::test]
    async fn caches_summary_and_extraction_without_repeating_model_calls() {
        let db = Database::new_in_memory().unwrap();
        seed(&db, 1, PDF);
        let model = FakeModel::new(false);
        let run = summarize_versions(&db, &[1], &model).await.unwrap();
        assert!(run.failures.is_empty());
        assert_eq!(run.summaries.len(), 1);
        assert!(selected_versions(&db, 10, None, false).unwrap().is_empty());
        // An explicit version displays the saved result even without another API call.
        let ids = selected_versions(&db, 10, Some(1), false).unwrap();
        assert_eq!(
            summarize_versions(&db, &ids, &model)
                .await
                .unwrap()
                .summaries
                .len(),
            1
        );
        assert_eq!(model.calls.get(), 1);
        let row: (i64, String, Option<i64>) = db
            .query_row(
                "SELECT Attempts, PagesJson, CompletedAt FROM ConditionsSummaries",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0, 1);
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&row.1).unwrap().len(),
            6
        );
        assert!(row.2.is_some());
    }

    #[tokio::test]
    async fn rejected_evidence_is_saved_for_review_but_never_cached_as_a_summary() {
        struct FabricatedQuote;
        impl SummaryModel for FabricatedQuote {
            async fn summarize(&self, _input: &SummaryInput) -> Result<String> {
                Ok(answer(&pages()))
            }
        }
        let db = Database::new_in_memory().unwrap();
        seed(&db, 1, PDF);
        let run = summarize_versions(&db, &[1], &FabricatedQuote)
            .await
            .unwrap();
        assert!(run.summaries.is_empty());
        assert_eq!(run.failures.len(), 1);
        let saved: (String, Option<String>, Option<i64>, String) = db
            .query_row(
                "SELECT RawResponse, SummaryJson, CompletedAt, LastError FROM ConditionsSummaries",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(saved.0, answer(&pages()));
        assert_eq!(saved.1, None);
        assert_eq!(saved.2, None);
        assert!(saved.3.contains("Evidence does not match PDF page"));
        assert_eq!(selected_versions(&db, 10, None, false).unwrap(), vec![1]);
    }

    #[tokio::test]
    async fn retries_failures_then_stops_until_explicitly_requested() {
        let db = Database::new_in_memory().unwrap();
        seed(&db, 1, PDF);
        let model = FakeModel::new(true);
        for attempt in 1..=MAX_ATTEMPTS {
            let ids = selected_versions(&db, 10, None, false).unwrap();
            let run = summarize_versions(&db, &ids, &model).await.unwrap();
            assert_eq!(run.failures.len(), 1);
            let saved: (i64, Option<String>, String) = db
                .query_row(
                    "SELECT Attempts, SummaryJson, LastError FROM ConditionsSummaries",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(saved.0, attempt);
            assert_eq!(saved.1, None);
            assert!(saved.2.contains("simulated model failure"));
        }
        assert!(selected_versions(&db, 10, None, false).unwrap().is_empty());
        assert!(selected_versions(&db, 10, Some(1), false)
            .unwrap()
            .is_empty());
        model.fail.set(false);
        let ids = selected_versions(&db, 10, None, true).unwrap();
        let run = summarize_versions(&db, &ids, &model).await.unwrap();
        assert_eq!(run.summaries.len(), 1);
        let error: Option<String> = db
            .query_row("SELECT LastError FROM ConditionsSummaries", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(error, None);
    }

    #[tokio::test]
    async fn one_bad_pdf_does_not_block_other_documents_or_call_the_model() {
        let db = Database::new_in_memory().unwrap();
        seed(&db, 1, b"invalid pdf");
        seed(&db, 2, PDF);
        let model = FakeModel::new(false);
        let run = summarize_versions(&db, &[1, 2], &model).await.unwrap();
        assert_eq!(run.failures.len(), 1);
        assert_eq!(run.summaries.len(), 1);
        assert_eq!(model.calls.get(), 1);
    }

    #[tokio::test]
    async fn new_document_and_prompt_versions_are_independent_and_batch_is_bounded() {
        let db = Database::new_in_memory().unwrap();
        seed(&db, 1, PDF);
        seed(&db, 2, PDF);
        assert_eq!(selected_versions(&db, 1, None, false).unwrap(), vec![2]);
        let model = FakeModel::new(false);
        summarize_versions(&db, &[2], &model).await.unwrap();
        db.execute("UPDATE ConditionsSummaries SET PromptVersion = 0", [])
            .unwrap();
        assert_eq!(selected_versions(&db, 2, None, false).unwrap(), vec![2, 1]);
        summarize_versions(&db, &[2], &model).await.unwrap();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM ConditionsSummaries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(model.calls.get(), 2);
        assert!(selected_versions(&db, 0, None, false).is_err());
        assert!(selected_versions(&db, 101, None, false).is_err());
        assert!(selected_versions(&db, 10, Some(900), false).is_err());
    }

    #[test]
    fn schema_upgrade_does_not_change_archived_pdfs_or_approval_history() {
        let db = Database::new_in_memory().unwrap();
        seed(&db, 1, PDF);
        initialize_schema(&db).unwrap();
        let bytes: Vec<u8> = db
            .query_row("SELECT Content FROM ApprovalDocumentVersions", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(bytes, PDF);
    }

    #[tokio::test]
    async fn openai_adapter_sends_schema_and_page_labels_and_parses_response() {
        check_openai_adapter("completed").await;
    }

    #[tokio::test]
    async fn openai_adapter_rejects_incomplete_response_even_with_valid_json() {
        check_openai_adapter("incomplete").await;
    }

    async fn check_openai_adapter(status: &'static str) {
        use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let (headers_len, content_len) = loop {
                let mut chunk = [0; 4096];
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
                if let Some(index) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..index]);
                    assert!(headers.starts_with("POST /v1/responses"));
                    let len: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_string)
                        })
                        .unwrap()
                        .parse()
                        .unwrap();
                    break (index + 4, len);
                }
            };
            while request.len() < headers_len + content_len {
                let mut chunk = [0; 4096];
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
            }
            let body: serde_json::Value =
                serde_json::from_slice(&request[headers_len..headers_len + content_len]).unwrap();
            assert_eq!(body["model"], MODEL);
            assert!(body.get("max_tokens").is_none());
            assert_eq!(body["max_output_tokens"], 5000);
            assert_eq!(body["reasoning"]["effort"], "low");
            assert_eq!(body["store"], false);
            assert_eq!(body["text"]["format"]["strict"], true);
            assert_eq!(body["text"]["format"]["schema"], response_schema());
            assert_eq!(body["instructions"], PROMPT);
            let source: serde_json::Value =
                serde_json::from_str(body["input"][0]["content"].as_str().unwrap()).unwrap();
            assert_eq!(source["pages"][0]["page"], 1);
            assert_eq!(source["pages"][0]["text"], pages()[0]);
            assert_eq!(source["overview_character_limit"], 273);
            let response = json!({"id":"local-test", "status":status, "model":MODEL,
                "incomplete_details": {"reason":"max_output_tokens"},
                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":answer(&pages())}]}]}).to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
        });
        let client = genai::Client::builder()
            .with_auth_resolver(AuthResolver::from_resolver_fn(|_| {
                Ok(Some(AuthData::from_single("fake-local-test-key")))
            }))
            .with_service_target_resolver(ServiceTargetResolver::from_resolver_fn(
                move |mut target: genai::ServiceTarget| {
                    target.endpoint = Endpoint::from_owned(endpoint.clone());
                    Ok(target)
                },
            ))
            .build();
        let input = SummaryInput {
            project_name: "Renfrew childcare".into(),
            source_url: "https://example.com/letter".into(),
            pages: pages(),
        };
        let result = OpenAiSummarizer(client).summarize(&input).await;
        if status == "completed" {
            assert_eq!(
                parse_summary(&result.unwrap(), &input.pages)
                    .unwrap()
                    .requirements
                    .len(),
                1
            );
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("was not completed"));
        }
        server.await.unwrap();
    }
}
