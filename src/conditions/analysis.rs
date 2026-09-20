//! Bounded select -> draft -> independent review workflow. Every stage uses the
//! configured generator model; evaluation labels are never supplied here.
use super::{format_post, overview_budget, parse_summary, ConditionsSummary, Requirement};
use anyhow::{bail, ensure, Context, Result};
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponseFormat, JsonSpec, StopReason,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const SELECT: &str = include_str!("select_prompt.txt");
const WRITE: &str = include_str!("write_prompt.txt");
const REVIEW: &str = include_str!("review_prompt.txt");
const VERIFY: &str = include_str!("verify_prompt.txt");
const TERMINOLOGY: &str = include_str!("terminology.txt");
const MAX_ROUNDS: usize = 4;
pub const DEFAULT_MODEL: &str = crate::llm::MODEL;
pub use crate::llm::require_api_key;

/// Identify the actual workflow, not just a manually incremented version number.
pub fn workflow_fingerprint() -> String {
    let mut hash = Sha256::new();
    for part in [
        SELECT,
        WRITE,
        REVIEW,
        VERIFY,
        TERMINOLOGY,
        include_str!("analysis.rs"),
        include_str!("../conditions.rs"),
        include_str!("../llm.rs"),
    ] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSpan {
    pub id: String,
    pub page: usize,
    pub text: String,
}

/// Stable IDs address text, so models never need to reproduce PDF typography.
pub fn source_spans(pages: &[String]) -> Vec<SourceSpan> {
    let mut spans = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let mut text = String::new();
        let mut number = 0;
        let mut flush = |text: &mut String| {
            if !text.is_empty() {
                number += 1;
                spans.push(SourceSpan {
                    id: format!("p{}s{}", index + 1, number),
                    page: index + 1,
                    text: std::mem::take(text),
                });
            }
        };
        // Keep paragraphs where possible; bound every citation for storage.
        for paragraph in page.split("\n\n") {
            for word in paragraph.split_whitespace() {
                if !text.is_empty() && text.len() + word.len() + 1 > 1000 {
                    flush(&mut text);
                }
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(word);
            }
            if text.len() >= 450 {
                flush(&mut text);
            }
        }
        flush(&mut text);
    }
    spans
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub stage: String,
    pub round: usize,
    pub elapsed_ms: u128,
    pub usage: Value,
    pub response: String,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Analysis {
    pub workflow_sha256: String,
    pub summary: Option<ConditionsSummary>,
    pub error: Option<String>,
    pub trace: Vec<Step>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Candidate {
    fact: String,
    has_unresolved_conflict: bool,
    reports_advisory_method: bool,
    source_ids: Vec<String>,
    stage: String,
    qualifications: String,
    #[serde(default)]
    editorial_reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Brief {
    project_label: String,
    candidates: Vec<Candidate>,
    lead_candidate_index: usize,
    lead_rationale: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Draft {
    text: String,
    candidate_indices: Vec<usize>,
}

#[derive(Debug, Deserialize)]
struct Drafts {
    drafts: Vec<Draft>,
}

#[derive(Debug, Deserialize)]
struct Verdict {
    index: usize,
    source_ids: Vec<String>,
    reports_advisory_method: bool,
    factual: bool,
    qualified: bool,
    #[serde(alias = "useful")]
    newsworthy: bool,
    readable: bool,
    issues: Vec<String>,
    #[serde(rename = "suggestions")]
    _suggestions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Review {
    #[serde(rename = "assessment")]
    _assessment: String,
    best_index: i64,
    reviews: Vec<Verdict>,
    repair_feedback: String,
}

#[derive(Debug, Deserialize)]
struct Verification {
    claims: Vec<VerifiedClaim>,
    meaning_changes: Vec<String>,
    assessment: String,
    accurate: bool,
    qualified: bool,
    concrete: bool,
    issues: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct VerifiedClaim {
    text: String,
    quotes: Vec<SourceQuote>,
    entailed: bool,
    explanation: String,
}

#[derive(Debug, Deserialize)]
struct SourceQuote {
    page: usize,
    text: String,
}

fn object(properties: Value) -> Value {
    let required: Vec<_> = properties.as_object().unwrap().keys().cloned().collect();
    json!({"type":"object", "properties":properties, "required":required, "additionalProperties":false})
}

fn array(items: Value) -> Value {
    json!({"type":"array", "items":items})
}

fn brief_schema() -> Value {
    object(json!({
        "project_label":{"type":"string"},
        "lead_candidate_index":{"type":"integer"}, "lead_rationale":{"type":"string"},
        "candidates":array(object(json!({
            "fact":{"type":"string"}, "source_ids":array(json!({"type":"string"})),
            "has_unresolved_conflict":{"type":"boolean"},
            "reports_advisory_method":{"type":"boolean"},
            "stage":{"type":"string"}, "qualifications":{"type":"string"},
            "editorial_reason":{"type":"string"}
        })))
    }))
}

fn drafts_schema() -> Value {
    object(json!({"drafts":array(object(json!({
        "text":{"type":"string"}, "candidate_indices":array(json!({"type":"integer"}))
    })))}))
}

fn review_schema() -> Value {
    let mut schema = object(json!({
        "assessment":{"type":"string"},
        "best_index":{"type":"integer"}, "repair_feedback":{"type":"string"},
        "reviews":array(object(json!({
            "index":{"type":"integer"}, "factual":{"type":"boolean"},
            "source_ids":array(json!({"type":"string"})),
            "reports_advisory_method":{"type":"boolean"},
            "qualified":{"type":"boolean"}, "newsworthy":{"type":"boolean"},
            "readable":{"type":"boolean"}, "issues":array(json!({"type":"string"})),
            "suggestions":array(json!({"type":"string"}))
        })))
    }));
    schema["properties"]["reviews"]["maxItems"] = json!(1);
    schema
}

#[derive(Clone, Copy)]
enum Stage {
    Select,
    Write,
    Review,
    Verify,
}

impl Stage {
    fn json_example(&self) -> Value {
        match self {
            Self::Verify => {
                json!({"claims":[{"text":"exact fragment of condition_text", "quotes":[{"page":1,"text":"verbatim source quotation"}],"entailed":false,"explanation":"Does the quotation actually establish every part of this claim?"}],"meaning_changes":[],"assessment":"claim-by-claim comparison with source", "accurate":false, "qualified":false, "concrete":false, "issues":["material discrepancy if any"]})
            }
            Self::Select => {
                json!({"project_label":"address and type", "lead_candidate_index":0, "lead_rationale":"source-based reason for this lead", "candidates":[{"fact":"one source-backed condition", "has_unresolved_conflict":false, "reports_advisory_method":false, "source_ids":["p1s1"], "stage":"stage from source", "qualifications":"material qualifications from source", "editorial_reason":"why this matters"}]})
            }
            Self::Write => {
                json!({"drafts":[{"text":"A complete concise sentence.", "candidate_indices":[0]}]})
            }
            Self::Review => {
                json!({"assessment":"source-based audit of the chosen draft", "best_index":0, "repair_feedback":"", "reviews":[{"index":0, "source_ids":["p1s1"], "reports_advisory_method":false, "factual":true, "qualified":true, "newsworthy":true, "readable":true, "issues":[], "suggestions":[]}]})
            }
        }
    }

    fn config(&self) -> (&'static str, &'static str, Value) {
        match self {
            Self::Select => ("select_conditions", SELECT, brief_schema()),
            Self::Write => ("write_conditions", WRITE, drafts_schema()),
            Self::Review => ("review_conditions", REVIEW, review_schema()),
            Self::Verify => (
                "verify_conditions",
                VERIFY,
                object(
                    json!({"claims":array(object(json!({"text":{"type":"string"},"quotes":array(object(json!({"page":{"type":"integer"},"text":{"type":"string"}}))),"entailed":{"type":"boolean"},"explanation":{"type":"string"}}))),"meaning_changes":array(json!({"type":"string"})),"assessment":{"type":"string"},"accurate":{"type":"boolean"},"qualified":{"type":"boolean"},"concrete":{"type":"boolean"},"issues":array(json!({"type":"string"}))}),
                ),
            ),
        }
    }
}

async fn complete(
    client: &genai::Client,
    model: &str,
    stage: Stage,
    mut input: Value,
    round: usize,
    trace: &mut Vec<Step>,
) -> Result<Value> {
    let first = complete_once(client, model, stage, input.clone(), round, trace).await;
    match first {
        Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => {
            input["format_feedback"] = json!(format!("Your {} response failed JSON/required-field validation: {error:#}. Return one JSON object using ONLY this stage's exact key structure. Include all required fields on every entry. Do not discuss or correct the format outside that object.", stage.config().0));
            complete_once(client, model, stage, input, round, trace).await
        }
        result => result,
    }
}

async fn complete_once(
    client: &genai::Client,
    model: &str,
    stage: Stage,
    input: Value,
    round: usize,
    trace: &mut Vec<Step>,
) -> Result<Value> {
    let example = stage.json_example();
    let (stage, system, schema) = stage.config();
    let system = if stage == "verify_conditions" {
        format!("{system}\n\n{TERMINOLOGY}")
    } else {
        system.to_owned()
    };
    // The official Z.ai endpoint supports JSON mode, but not schema-constrained
    // decoding. Give it an object example rather than schema syntax it may echo.
    // Required fields (especially verdict booleans) still fail closed locally.
    let system = if model == DEFAULT_MODEL {
        format!("{system}\nReturn ONLY a JSON object with this exact key structure, no schema or properties wrapper:\n{example}\nThe example is a FORMAT TEMPLATE, not facts or verdicts. Replace all values using the input. Include EVERY key shown on EVERY array entry; all boolean fields are required. Use actual source IDs and indices. Include the requested number of entries. For review, set each boolean independently from the source, use an empty issues array for passing drafts, and select a passing best_index or -1 if none pass.")
    } else {
        system.to_owned()
    };
    let started = Instant::now();
    let mut options = ChatOptions::default()
        .with_capture_raw_body(true)
        .with_max_tokens(
            if matches!(stage, "review_conditions" | "verify_conditions") {
                10000
            } else {
                6000
            },
        )
        .with_response_format(if model == DEFAULT_MODEL {
            ChatResponseFormat::JsonMode
        } else {
            JsonSpec::new(stage, schema).into()
        });
    options = options.with_extra_body(json!({
        "reasoning":{"effort":if matches!(stage, "review_conditions" | "verify_conditions") {"high"} else {"medium"}},
        "provider":crate::llm::provider_options(model)?
    }));
    let response = tokio::time::timeout(
        Duration::from_secs(120),
        client.exec_chat(
            model,
            ChatRequest::new(vec![ChatMessage::user(input.to_string())]).with_system(system),
            Some(&options),
        ),
    )
    .await;
    let mut step = Step {
        stage: stage.into(),
        round,
        elapsed_ms: started.elapsed().as_millis(),
        usage: Value::Null,
        response: String::new(),
        error: None,
        decision: None,
    };
    let result = (|| {
        let response = response.context("Model request timed out")??;
        step.usage = serde_json::to_value(&response.usage)?;
        step.usage["resolved_model"] = serde_json::to_value(&response.model_iden)?;
        step.usage["provider_model"] = serde_json::to_value(&response.provider_model_iden)?;
        if let Some(raw) = &response.captured_raw_body {
            step.usage["provider_usage"] = raw["usage"].clone();
            step.usage["generation_id"] = raw["id"].clone();
            step.usage["provider"] = raw["provider"].clone();
        }
        step.response = response.first_text().unwrap_or_default().to_owned();
        ensure!(
            matches!(response.stop_reason, Some(StopReason::Completed(_))),
            "Incomplete model response: {:?}",
            response.stop_reason
        );
        let value: Value = serde_json::from_str(&step.response).context("Invalid model JSON")?;
        match stage {
            "select_conditions" => {
                serde_json::from_value::<Brief>(value.clone())?;
            }
            "write_conditions" => {
                serde_json::from_value::<Drafts>(value.clone())?;
            }
            "review_conditions" => {
                serde_json::from_value::<Review>(value.clone())?;
            }
            "verify_conditions" => {
                serde_json::from_value::<Verification>(value.clone())?;
            }
            _ => unreachable!(),
        }
        Ok(value)
    })();
    if let Err(ref error) = result {
        step.error = Some(format!("{error:#}"));
    }
    trace.push(step);
    result
}

fn validate_brief(brief: &Brief, spans: &[SourceSpan]) -> Result<()> {
    ensure!(
        !brief.project_label.trim().is_empty()
            && brief.project_label.chars().count() <= 65
            && !brief.project_label.chars().any(char::is_control),
        "Project label must be a short address and type, at most 65 characters"
    );
    ensure!(
        (1..=8).contains(&brief.candidates.len()),
        "Expected 1–8 candidate facts"
    );
    for candidate in &brief.candidates {
        ensure!(!candidate.fact.trim().is_empty(), "Empty candidate fact");
        ensure!(
            (1..=8).contains(&candidate.source_ids.len()),
            "Expected 1–8 source IDs per candidate"
        );
        for id in &candidate.source_ids {
            ensure!(spans.iter().any(|s| &s.id == id), "Unknown source ID: {id}");
        }
    }
    ensure!(
        brief.lead_candidate_index < brief.candidates.len(),
        "Unknown lead candidate"
    );
    ensure!(
        !brief.candidates[brief.lead_candidate_index].has_unresolved_conflict,
        "Selected lead has an unresolved conflict; choose a different, unambiguous condition"
    );
    Ok(())
}

fn materialize(
    draft: &Draft,
    brief: &Brief,
    spans: &[SourceSpan],
    pages: &[String],
    url: &str,
) -> Result<ConditionsSummary> {
    ensure!(
        !draft.text.to_lowercase().contains("approval conditions:")
            && !draft
                .text
                .to_lowercase()
                .contains(&brief.project_label.to_lowercase()),
        "The app already supplies the project introduction; return only the condition sentence"
    );
    ensure!(
        !regex::Regex::new(r"\b(?:VBBL|SRA|POPS|SRW|[FSNAE][1-4])\b").unwrap().is_match(&draft.text),
        "Replace municipal acronyms and code-level labels with the concrete requirement in plain English"
    );
    ensure!(
        !regex::Regex::new(r"(?i)\b(?:section\s+\d|public life use\b|(?:design|upgrade) levels?\b|levels?\s*[-:]?\s*[1-4]\b)").unwrap().is_match(&draft.text),
        "Explain the concrete demand in ordinary language, not a numbered document reference or legal phrase. Say what the public can use or what the applicant must change. If the source gives only an unexplained external reference, choose a different substantive condition"
    );
    ensure!(
        !regex::Regex::new(r"(?i)\b(?:any|every)\s+(?:(?:building|development)\s+)?permits?\b").unwrap().is_match(&draft.text),
        "Do not generalize a deadline to any/every permit. Name the specific permit and purpose given by the source, or omit the deadline while reporting the substantive demand"
    );
    ensure!(
        !regex::Regex::new(r"(?i)\b(?:at|by)\s+(?:the\s+)?(?:(?:building|development)\s+)?permit(?:\s+application)?\s+stage\b").unwrap().is_match(&draft.text),
        "Avoid ambiguous at/by permit stage timing. For later review, say Building review calls for, without implying installed work at permit application"
    );
    ensure!(
        draft.candidate_indices == [brief.lead_candidate_index],
        "Draft must cite only the selected lead fact"
    );
    let mut requirements = Vec::new();
    ensure!(
        draft
            .candidate_indices
            .contains(&brief.lead_candidate_index),
        "Draft must retain the selected lead fact"
    );
    let mut used = HashSet::new();
    for &index in &draft.candidate_indices {
        ensure!(used.insert(index), "Duplicate candidate index");
        let candidate = brief
            .candidates
            .get(index)
            .context("Draft cites an unknown candidate")?;
        let anchors = concrete_anchors(&candidate.fact);
        ensure!(
            !has_opaque_levels(&candidate.fact) || anchors.iter().any(|term| draft.text.to_lowercase().contains(term)),
            "Report the actual named physical requirement ({anchors:?}), not a paraphrase of code upgrade levels"
        );
        ensure!(
            !candidate.has_unresolved_conflict,
            "Draft uses a conflicting candidate; choose an unambiguous condition"
        );
        validate_advisory_wording(&draft.text, candidate.reports_advisory_method)?;
        for id in &candidate.source_ids {
            let span = spans
                .iter()
                .find(|s| &s.id == id)
                .context("Unknown source ID")?;
            requirements.push(Requirement {
                requirement: candidate.fact.clone(),
                page: span.page,
                evidence: span.text.clone(),
            });
        }
    }
    let summary = ConditionsSummary {
        overview: format!("{}{}", post_prefix(brief), draft.text),
        requirements,
        limitations: vec![],
    };
    let summary = parse_summary(&serde_json::to_string(&summary)?, pages)?;
    format_post(&summary, url)?;
    Ok(summary)
}

fn validate_advisory_wording(text: &str, reports_advisory_method: bool) -> Result<()> {
    let explicit = regex::Regex::new(
        r"(?i)\b(?:suggests?|suggested|recommends?|recommended|could|option|optional|possible)\b",
    )
    .unwrap();
    ensure!(!reports_advisory_method || explicit.is_match(text),
        "The reported design method is advisory. Describe it as a suggestion, or choose a clear requirement; do not make suggested design changes mandatory");
    Ok(())
}

fn post_prefix(brief: &Brief) -> String {
    format!(
        "Approval conditions: {}. ",
        brief.project_label.trim().trim_end_matches('.')
    )
}

fn has_opaque_levels(text: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:[FSNAE][1-4]|(?:design|upgrade) levels?)\b").unwrap()
    });
    PATTERN.is_match(text)
}

fn concrete_anchors(text: &str) -> Vec<&'static str> {
    let text = text.to_lowercase();
    [
        "sprinkler",
        "smoke detection",
        "co alarm",
        "emergency light",
        "separation",
        "structural assessment",
        "structural report",
        "structural design",
        "seismic retrofit",
        "ceiling",
        "sidewalk",
    ]
    .into_iter()
    .filter(|term| text.contains(term))
    .collect()
}

fn weak_daycare_administration(label: &str, fact: &str) -> bool {
    let label = label.to_lowercase();
    let fact = fact.to_lowercase();
    ["daycare", "childcare", "child care", "day care"]
        .iter()
        .any(|kind| label.contains(kind))
        && ((fact.contains("hour")
            && fact.contains("operat")
            && (fact.contains("weekday") || (fact.contains("monday") && fact.contains("friday"))))
            || (fact.contains("capacity")
                && fact.contains("final")
                && (fact.contains("inspection") || fact.contains("licens"))))
}

fn reviewed_evidence(
    summary: &mut ConditionsSummary,
    verdict: &Verdict,
    draft: &Draft,
    spans: &[SourceSpan],
    pages: &[String],
) -> Result<()> {
    ensure!(
        (1..=8).contains(&verdict.source_ids.len()),
        "Review must cite 1–8 supporting source IDs"
    );
    let mut seen = HashSet::new();
    let mut cited_pages = HashSet::new();
    let mut requirements = vec![];
    for id in &verdict.source_ids {
        ensure!(seen.insert(id), "Duplicate reviewed source ID");
        let span = spans
            .iter()
            .find(|s| &s.id == id)
            .context("Review cites unknown source ID")?;
        // Keep the full cited page: arbitrary span boundaries can separate a
        // heading from its subclause or a requirement from its qualification.
        // The reviewer still chooses source IDs; no model-written quote is stored.
        if cited_pages.insert(span.page) {
            requirements.push(Requirement {
                requirement: draft.text.clone(),
                page: span.page,
                evidence: super::normalized(&pages[span.page - 1]),
            });
        }
    }
    summary.requirements = requirements;
    validate_advisory_wording(&draft.text, verdict.reports_advisory_method)?;
    parse_summary(&serde_json::to_string(summary)?, pages)?;
    Ok(())
}

fn accepted_index(review: &Review, valid: &[usize]) -> Result<usize> {
    let index = usize::try_from(review.best_index).context("Reviewer rejected all drafts")?;
    ensure!(valid.contains(&index), "Reviewer chose an invalid draft");
    ensure!(
        review.reviews.iter().filter(|r| r.index == index).count() == 1,
        "Return exactly one verdict for the chosen draft"
    );
    let verdict = review
        .reviews
        .iter()
        .find(|r| r.index == index)
        .context("Missing verdict")?;
    ensure!(
        verdict.factual
            && verdict.qualified
            && verdict.newsworthy
            && verdict.readable
            && verdict.issues.is_empty(),
        "Selected draft did not pass all review criteria"
    );
    Ok(index)
}

fn validate_claims(
    verification: &Verification,
    body: &str,
    pages: &[String],
    available: &[usize],
) -> Result<()> {
    ensure!(
        (1..=8).contains(&verification.claims.len()),
        "Final audit must ground 1–8 claims"
    );
    let body = super::normalized(body);
    let source_choice = regex::Regex::new(r"(?i)\b([a-z-]{3,})\s+or\s+([a-z-]{3,})\b").unwrap();
    let explicit_choice =
        regex::Regex::new(r"(?i)\b(?:or|alternatives?)\b|\b(?:can|may) accept\b").unwrap();
    let body_words: Vec<_> = body
        .to_lowercase()
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect();
    let names_arm = |arm: &str| {
        let root = arm.chars().take(5).collect::<String>().to_lowercase();
        body_words.iter().any(|w| w.starts_with(&root))
    };
    let deadline = regex::Regex::new(r"(?i)\b(?:before|prior to|ahead of|until|by|precondition|prerequisite)\b|\bfor (?:the |a )?(?:building|development) permit\b").unwrap();
    let permit_purpose =
        regex::Regex::new(r"(?i)\bpermit\s+(?:to|for)\s+(?:allow\s+(?:for\s+)?)?([a-z]{4,})")
            .unwrap();
    let mut covered = String::new();
    for claim in &verification.claims {
        let text = super::normalized(&claim.text);
        ensure!(
            !text.is_empty() && body.contains(&text),
            "Audited claim must be an exact fragment of the condition text"
        );
        ensure!(
            claim.entailed && !claim.explanation.trim().is_empty(),
            "Source does not entail claim {:?}: {}",
            claim.text,
            claim.explanation
        );
        ensure!(
            (1..=8).contains(&claim.quotes.len()),
            "Each audited claim needs source quotations"
        );
        for quote in &claim.quotes {
            ensure!(
                available.contains(&quote.page),
                "Audit quote must use a supplied source page"
            );
            let text = super::normalized(&quote.text);
            let source_page = super::normalized(&pages[quote.page - 1]);
            let context = quote_context(&source_page, &text)?;
            ensure!(
                !body.to_lowercase().contains("rental agreement") || !context.to_lowercase().contains("housing agreement"),
                "Keep Housing Agreement as the legal instrument; rental agreement can misleadingly describe a tenancy lease"
            );
            let source_text = super::normalized(&pages[quote.page - 1]).to_lowercase();
            for broader in ["amplified sound", "amplified audio"] {
                ensure!(
                    !body.to_lowercase().contains(broader) || !source_text.contains("amplified music") || source_text.contains(broader),
                    "A ban on amplified music does not establish a ban on all amplified sound; retain the source's narrower scope"
                );
            }
            ensure!(
                explicit_choice.is_match(&body) || !source_choice.captures_iter(context).any(|pair| names_arm(&pair[1]) != names_arm(&pair[2])),
                "The quoted source offers an OR alternative. Preserve that choice explicitly in the condition text, or select a narrower independent demand with its own supporting quotation"
            );
            ensure!(
                !deadline.is_match(&body) || !context.to_lowercase().contains("arrangements") || body.to_lowercase().contains("arrang"),
                "The source requires arrangements before a permit, not completed authorization or registration. Preserve arrangements or omit the deadline"
            );
            for purpose in permit_purpose.captures_iter(context) {
                ensure!(!deadline.is_match(&body) || names_arm(&purpose[1]),
                    "The quoted deadline concerns a permit for a specific purpose ({0}); preserve that purpose or omit the deadline", &purpose[1]);
            }
            ensure!(
                text.chars().count() >= 15
                    && super::normalized(&pages[quote.page - 1]).contains(&text),
                "Audit quotation is not verbatim on page {}",
                quote.page
            );
        }
        covered.push(' ');
        covered.push_str(&text);
    }
    // The model must not quietly omit a difficult clause from its claim audit.
    // This checks coverage, not entailment; semantic support remains a model judgment.
    let words = |text: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect::<HashSet<_>>()
    };
    let covered = words(&covered);
    let factual_body = regex::Regex::new(r"(?i)^(?:(?:(?:early|later|preliminary)\s+)?building review|(?:approval\s+)?conditions|review comments)\s+(?:calls? for|requires?|includes?|flags?)\s+")
        .unwrap()
        .replace(&body, "");
    let missing: Vec<_> = words(&factual_body)
        .difference(&covered)
        .filter(|w| {
            ![
                "and", "or", "plus", "the", "a", "an", "with", "to", "of", "in", "on", "for", "as",
                "by", "at", "from", "must", "shall",
            ]
            .contains(&w.as_str())
        })
        .cloned()
        .collect();
    ensure!(
        missing.is_empty(),
        "Final audit omitted words from the condition claim: {missing:?}"
    );
    Ok(())
}

/// Use original sentence context so a clipped quotation cannot hide a qualifier.
fn quote_context<'a>(page: &'a str, quote: &str) -> Result<&'a str> {
    let start = page
        .find(quote)
        .context("Audit quotation does not occur on its source page")?;
    let end = start + quote.len();
    let sentence_start = page[..start].rfind(". ").map_or(0, |at| at + 2);
    let sentence_end = if quote.ends_with('.') {
        end
    } else {
        page[end..].find(". ").map_or(page.len(), |at| end + at + 1)
    };
    Ok(&page[sentence_start..sentence_end])
}

fn resolve_audit_quotes(
    verification: &mut Verification,
    pages: &[String],
    available: &[usize],
) -> Result<Vec<String>> {
    let mut corrections = Vec::new();
    for claim in &mut verification.claims {
        for quote in &mut claim.quotes {
            let text = super::normalized(&quote.text);
            ensure!(text.chars().count() >= 15, "Audit quotation is too short");
            if available.contains(&quote.page)
                && super::normalized(&pages[quote.page - 1]).contains(&text)
            {
                continue;
            }
            let matches: Vec<_> = available
                .iter()
                .copied()
                .filter(|&page| super::normalized(&pages[page - 1]).contains(&text))
                .collect();
            ensure!(
                matches.len() == 1,
                "Audit quotation has no unambiguous verbatim match on supplied pages"
            );
            corrections.push(format!(
                "Corrected audit quotation page {} to {} by unique verbatim source match: {}",
                quote.page, matches[0], text
            ));
            quote.page = matches[0];
        }
    }
    Ok(corrections)
}

async fn workflow(
    client: &genai::Client,
    model: &str,
    name: &str,
    url: &str,
    pages: &[String],
    trace: &mut Vec<Step>,
) -> Result<ConditionsSummary> {
    crate::llm::validate_model(model)?;
    ensure!(
        overview_budget(url) >= 40,
        "Source URL leaves too little room for a post"
    );
    let spans = source_spans(pages);
    ensure!(!spans.is_empty(), "No source passages");
    let mut feedback = String::new();
    for round in 1..=MAX_ROUNDS {
        let result: Result<ConditionsSummary> = async {
            let mut brief: Brief = serde_json::from_value(complete(client, model, Stage::Select,
                json!({"project_name":name, "sources":spans, "previous_feedback":feedback}), round, trace).await?)?;
            validate_brief(&brief, &spans)?;
            let lead = &brief.candidates[brief.lead_candidate_index];
            if (has_opaque_levels(&lead.fact) && concrete_anchors(&lead.fact).is_empty()) || weak_daycare_administration(&brief.project_label, &lead.fact) {
                let previous = brief.lead_candidate_index;
                brief.lead_candidate_index = brief.candidates.iter().position(|c| !c.has_unresolved_conflict && !c.reports_advisory_method && (!has_opaque_levels(&c.fact) || !concrete_anchors(&c.fact).is_empty()) && !weak_daycare_administration(&brief.project_label, &c.fact))
                    .context("Every candidate uses unexplained code levels, conflict or advice; select a concrete independent requirement")?;
                if let Some(last) = trace.last_mut() { last.decision = Some(format!("Lead {previous} offers only code levels or routine daycare hours or final-capacity confirmation; selected the first eligible ranked candidate {} instead.", brief.lead_candidate_index)); }
            }
            let body_budget = overview_budget(url).saturating_sub(post_prefix(&brief).chars().count());
            ensure!(body_budget >= 40, "Project label and link leave too little room for a condition");
            let mut drafts = Drafts { drafts: vec![] };
            let mut valid = Vec::new();
            let mut errors = Vec::new();
            let mut writing_feedback = feedback.clone();
            // Let the writer use measured lengths to repair a draft before
            // paying to read and select from the entire document again.
            for attempt in 0..2 {
                drafts = serde_json::from_value(complete(client, model, Stage::Write,
                    json!({"selected_candidate":brief.candidates[brief.lead_candidate_index], "selected_candidate_index":brief.lead_candidate_index, "required_concrete_terms":if has_opaque_levels(&brief.candidates[brief.lead_candidate_index].fact) {concrete_anchors(&brief.candidates[brief.lead_candidate_index].fact)} else {vec![]}, "prefix_already_supplied":post_prefix(&brief), "character_budget":body_budget, "target_words":if attempt == 0 {(body_budget / 7).min(24)} else {(body_budget / 10).min(16)}, "previous_feedback":writing_feedback}), round, trace).await?)?;
                ensure!((1..=3).contains(&drafts.drafts.len()), "Expected 1–3 drafts");
                errors.clear();
                for (index, draft) in drafts.drafts.iter().enumerate() {
                    match materialize(draft, &brief, &spans, pages, url) {
                        Ok(_) => valid.push(index),
                        Err(e) => errors.push(format!("Draft {index}: {} characters, maximum {body_budget}, {} over budget. {e:#}. Text: {}", draft.text.chars().count(), draft.text.chars().count().saturating_sub(body_budget), draft.text)),
                    }
                }
                if !valid.is_empty() { break; }
                writing_feedback = format!("{}\nAll drafts failed. Rewrite substantially shorter, retaining essential qualifications. Report a narrower part of the selected demand instead of enumerating every detail. Measured validation results: {}", feedback, errors.join("; "));
                if let Some(last) = trace.last_mut() { last.error = Some(writing_feedback.clone()); }
            }
            ensure!(!valid.is_empty(), "All drafts failed deterministic checks: {}", errors.join("; "));
            let mut audit_failures = Vec::new();
            while !valid.is_empty() {
            let review: Review = serde_json::from_value(complete(client, model, Stage::Review,
                json!({"project_name":name, "sources":spans,
                    "drafts":valid.iter().map(|&index| json!({"index":index,"text":format!("{}{}",post_prefix(&brief),drafts.drafts[index].text),
                        "candidate_source_ids":drafts.drafts[index].candidate_indices.iter().flat_map(|&i| brief.candidates[i].source_ids.iter()).collect::<Vec<_>>()
                    })).collect::<Vec<_>>() }), round, trace).await?)?;
            let index = accepted_index(&review, &valid).with_context(|| {
                let blockers: Vec<_> = review.reviews.iter().flat_map(|r| &r.issues).collect();
                format!("{}; blocking issues: {:?}", review.repair_feedback, blockers)
            })?;
            let audited: Result<ConditionsSummary> = async {
            let mut summary = materialize(&drafts.drafts[index], &brief, &spans, pages, url)?;
            let verdict = review.reviews.iter().find(|r| r.index == index).context("Missing review")?;
            reviewed_evidence(&mut summary, verdict, &drafts.drafts[index], &spans, pages)?;
            // A fresh audit sees one post and complete cited pages, without the
            // selector's rationale or the editor's verdict to anchor its answer.
            // Page one supplies the address, project type and approval context.
            let mut evidence_pages: Vec<_> = std::iter::once(1).chain(summary.requirements.iter().map(|r| r.page)).collect();
            evidence_pages.sort_unstable();
            evidence_pages.dedup();
            let mut verification: Verification = serde_json::from_value(complete(client, model, Stage::Verify,
                json!({"post":summary.overview,"condition_text":drafts.drafts[index].text,"source_pages":evidence_pages.iter().map(|&page| json!({"page":page,"text":pages[page-1]})).collect::<Vec<_>>() }), round, trace).await?)?;
            ensure!(verification.accurate && verification.qualified && verification.concrete && verification.issues.is_empty() && verification.meaning_changes.is_empty(),
                "Final source audit failed: {}; meaning changes: {:?}; issues: {:?}", verification.assessment, verification.meaning_changes, verification.issues);
            let corrections = resolve_audit_quotes(&mut verification, pages, &evidence_pages)?;
            if !corrections.is_empty() {
                if let Some(last) = trace.last_mut() { last.decision = Some(corrections.join("\n")); }
            }
            validate_claims(&verification, &drafts.drafts[index].text, pages, &evidence_pages)?;
            Ok(summary)
            }.await;
            match audited {
                Ok(summary) => return Ok(summary),
                Err(error) => {
                    let message = format!("{error:#}");
                    if let Some(last) = trace.last_mut() {
                        if last.error.is_none() { last.error = Some(message.clone()); }
                    }
                    // Auth/quota errors must stop the run, not try another draft.
                    if provider_rejected(&message) { bail!(message); }
                    audit_failures.push(format!("Draft {index}: {message}"));
                    valid.retain(|&other| other != index);
                }
            }
            }
            bail!("All reviewed drafts failed final source audit: {}", audit_failures.join("; "))
        }.await;
        match result {
            Ok(summary) => return Ok(summary),
            Err(error) => {
                feedback = if error.downcast_ref::<serde_json::Error>().is_some() {
                    "A stage failed to produce valid JSON after a format retry. Follow your own stage's response format; select a concrete source-backed condition.".into()
                } else {
                    format!("{error:#}")
                };
                if let Some(last) = trace.last_mut() {
                    if last.error.is_none() {
                        last.error = Some(feedback.clone());
                    }
                }
                if provider_rejected(&feedback) {
                    bail!("Provider rejected further requests: {feedback}");
                }
            }
        }
    }
    bail!("No acceptable conditions post after {MAX_ROUNDS} rounds: {feedback}")
}

fn provider_rejected(message: &str) -> bool {
    [
        "credit_balance_exhausted",
        "insufficient_quota",
        "402 Payment Required",
        "401 Unauthorized",
    ]
    .iter()
    .any(|code| message.contains(code))
}

/// Model-independent entry point shared by the application and live eval runner.
pub async fn analyze_pages(
    client: &genai::Client,
    model: &str,
    name: &str,
    url: &str,
    pages: &[String],
) -> Analysis {
    let mut trace = Vec::new();
    let result = workflow(client, model, name, url, pages, &mut trace).await;
    match result {
        Ok(summary) => Analysis {
            workflow_sha256: workflow_fingerprint(),
            summary: Some(summary),
            error: None,
            trace,
        },
        Err(error) => Analysis {
            workflow_sha256: workflow_fingerprint(),
            summary: None,
            error: Some(format!("{error:#}")),
            trace,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::normalized;
    use super::*;

    async fn mock_client(
        responses: Vec<Value>,
    ) -> (genai::Client, tokio::task::JoinHandle<Vec<Value>>) {
        use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = vec![];
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (start, len) = loop {
                    let mut buffer = [0; 4096];
                    let read = socket.read(&mut buffer).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(index) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..index]);
                        assert!(headers.starts_with("POST /v1/chat/completions"));
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_owned)
                            })
                            .unwrap()
                            .parse::<usize>()
                            .unwrap();
                        break (index + 4, len);
                    }
                };
                while bytes.len() < start + len {
                    let mut buffer = [0; 4096];
                    let read = socket.read(&mut buffer).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&buffer[..read]);
                }
                let request: Value = serde_json::from_slice(&bytes[start..start + len]).unwrap();
                {
                    assert_eq!(request["model"], "z-ai/glm-5.3-flash");
                    assert_eq!(
                        request["provider"]["max_price"],
                        json!({"prompt":0.15,"completion":0.50,"request":0})
                    );
                    assert_eq!(request["provider"]["require_parameters"], true);
                    assert_eq!(request["provider"]["only"], json!(["z-ai/fp8"]));
                    assert_eq!(request["provider"]["allow_fallbacks"], false);
                    assert_eq!(request["response_format"]["type"], "json_object");
                    assert!(request["messages"][0]["content"]
                        .as_str()
                        .unwrap()
                        .contains("exact key structure"));
                    assert!(request.get("models").is_none());
                    assert!(request.get("reasoning_effort").is_none());
                    assert_eq!(
                        request["max_tokens"],
                        if request["reasoning"]["effort"] == "high" {
                            10000
                        } else {
                            6000
                        }
                    );
                }
                requests.push(request);
                let body = json!({"id":"test-openrouter", "model":"z-ai/glm-5.3-flash", "provider":"TestProvider", "usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30,"cost":0.0012},
                    "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":response.to_string()}}]}).to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        let client = genai::Client::builder()
            .with_auth_resolver(AuthResolver::from_resolver_fn(|_| {
                Ok(Some(AuthData::from_single("local-test-key")))
            }))
            .with_service_target_resolver(ServiceTargetResolver::from_resolver_fn(
                move |mut target: genai::ServiceTarget| {
                    target.endpoint = Endpoint::from_owned(endpoint.clone());
                    Ok(target)
                },
            ))
            .build();
        (client, server)
    }

    fn sample_responses(accepted: bool) -> Vec<Value> {
        let mut responses = vec![
            json!({"project_label":"1 Test St daycare", "lead_candidate_index":0,"lead_rationale":"A concrete quantity.","candidates":[{"fact":"Provide five Class B bicycle spaces.", "has_unresolved_conflict":false,"reports_advisory_method":false,"source_ids":["p1s1"], "stage":"revised drawings", "qualifications":"", "editorial_reason":"Concrete requirement."}]}),
            json!({"drafts":[{"text":"Provide five Class B bicycle spaces.", "candidate_indices":[0]}]}),
            json!({"assessment":"The source supports five bicycle spaces in revised drawings.","best_index":if accepted {0} else {-1}, "repair_feedback":if accepted {""} else {"Check the stage against p1s1."}, "reviews":[{
                "index":0,"source_ids":["p1s1"],"reports_advisory_method":false,"factual":true,"qualified":accepted,"newsworthy":true,"readable":true,
                "issues":if accepted {vec![]} else {vec!["Timing needs correction"]}, "suggestions":["Optional wording improvement"]}]}),
        ];
        if accepted {
            responses.push(json!({"claims":[{"text":"Provide five Class B bicycle spaces.","quotes":[{"page":1,"text":"five Class B bicycle spaces"}],"entailed":true,"explanation":"The source names five bicycle spaces."}],"meaning_changes":[],"assessment":"The source requires five bicycle spaces in revised plans, with no material omission.","accurate":true,"qualified":true,"concrete":true,"issues":[]}));
        }
        responses
    }

    #[tokio::test]
    async fn forbidden_models_fail_before_any_paid_request() {
        for model in [
            "gpt-6-astra",
            "open_router::openai/gpt-5",
            "open_router::openrouter/auto",
        ] {
            let result = analyze_pages(
                &genai::Client::default(),
                model,
                "Test",
                "https://example.com",
                &["Provide five bicycle spaces in revised drawings.".into()],
            )
            .await;
            assert!(result.summary.is_none());
            assert!(result.trace.is_empty());
            assert!(result.error.unwrap().contains("disabled"));
        }
    }

    #[tokio::test]
    async fn openrouter_uses_exact_model_and_preserves_provider_charge() {
        let (client, server) = mock_client(sample_responses(true)).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &["Provide five Class B bicycle spaces in revised plans.".into()],
        )
        .await;
        assert!(result.summary.is_some(), "{:?}", result.error);
        assert_eq!(server.await.unwrap().len(), 4);
        for step in result.trace {
            assert_eq!(step.usage["provider_usage"]["cost"], 0.0012);
            assert_eq!(step.usage["generation_id"], "test-openrouter");
            assert_eq!(step.usage["provider"], "TestProvider");
            assert!(step.usage["provider_model"]
                .to_string()
                .contains("z-ai/glm-5.3-flash"));
        }
    }

    #[test]
    fn final_claim_audit_rejects_invented_quotes_inferences_and_omitted_clauses() {
        let body = "Provide five Class B bicycle spaces.";
        let pages = vec!["Revised plans must provide five Class B bicycle spaces.".into()];
        let sample = sample_responses(true)[3].clone();
        let verification: Verification = serde_json::from_value(sample.clone()).unwrap();
        assert!(validate_claims(&verification, body, &pages, &[1]).is_ok());
        for (field, value) in [
            ("entailed", json!(false)),
            ("text", json!("Provide five")),
            (
                "quotes",
                json!([{"page":1,"text":"Ten bicycle spaces must be built."}]),
            ),
            (
                "quotes",
                json!([{"page":2,"text":"five Class B bicycle spaces"}]),
            ),
        ] {
            let mut invalid = sample.clone();
            invalid["claims"][0][field] = value;
            let verification: Verification = serde_json::from_value(invalid).unwrap();
            assert!(
                validate_claims(&verification, body, &pages, &[1]).is_err(),
                "{field}"
            );
        }
        let mut missing = sample;
        missing.as_object_mut().unwrap().remove("claims");
        assert!(serde_json::from_value::<Verification>(missing).is_err());
    }

    #[test]
    fn quoted_alternatives_cannot_become_one_mandatory_option() {
        let pages = vec!["Existing doors must be removed or relocated off City property.".into()];
        let mut response = sample_responses(true)[3].clone();
        response["claims"][0]["quotes"][0]["text"] = json!(pages[0]);
        for (text, expected) in [
            ("Existing doors must be relocated off City property.", false),
            (
                "Existing doors must be removed or relocated off City property.",
                true,
            ),
        ] {
            response["claims"][0]["text"] = json!(text);
            let verification: Verification = serde_json::from_value(response.clone()).unwrap();
            assert_eq!(
                validate_claims(&verification, text, &pages, &[1]).is_ok(),
                expected
            );
        }
    }

    #[test]
    fn audit_quotes_can_correct_only_unique_matches_on_supplied_pages() {
        let source = "Provide five Class B bicycle spaces.".to_owned();
        let pages = vec![
            "An unrelated introduction to the letter.".into(),
            source.clone(),
            source,
        ];
        let raw = sample_responses(true)[3].clone();
        let mut audit: Verification = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(
            resolve_audit_quotes(&mut audit, &pages, &[1, 2])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(audit.claims[0].quotes[0].page, 2);
        assert_eq!(
            raw["claims"][0]["quotes"][0]["page"], 1,
            "Raw response remains unchanged"
        );
        for available in [&[1][..], &[1, 2, 3][..]] {
            let mut audit: Verification = serde_json::from_value(raw.clone()).unwrap();
            assert!(resolve_audit_quotes(&mut audit, &pages, available).is_err());
        }
        audit.claims[0].quotes[0].text = "Provide ten Class B bicycle spaces.".into();
        assert!(resolve_audit_quotes(&mut audit, &pages, &[1, 2]).is_err());
    }

    #[test]
    fn clipped_quotes_cannot_hide_alternatives_or_deadline_purposes() {
        for (source, quote, body) in [
            (
                "Existing doors must be removed or relocated off City property.",
                "relocated off City property.",
                "Existing doors must be relocated off City property.",
            ),
            (
                "Consolidate the parcels before a building permit to relocate the house.",
                "Consolidate the parcels before a building permit",
                "Combining the parcels is a precondition for the building permit.",
            ),
        ] {
            let mut response = sample_responses(true)[3].clone();
            response["claims"][0]["text"] = json!(body);
            response["claims"][0]["quotes"][0]["text"] = json!(quote);
            let audit: Verification = serde_json::from_value(response).unwrap();
            assert!(validate_claims(&audit, body, &[source.into()], &[1]).is_err());
        }
    }

    #[test]
    fn audit_preserves_deadline_scope_without_requiring_irrelevant_source_words() {
        for (source, body, claim, expected) in [
            (
                "A Housing Agreement must secure all twelve homes as rental.",
                "A rental agreement must secure all twelve homes as rental.",
                "A rental agreement must secure all twelve homes as rental.",
                false,
            ),
            (
                "A Housing Agreement must secure all twelve homes as rental.",
                "A Housing Agreement must secure all twelve homes as rental.",
                "A Housing Agreement must secure all twelve homes as rental.",
                true,
            ),
            (
                "Provide five Class B bicycle spaces.",
                "Building review calls for five Class B bicycle spaces.",
                "five Class B bicycle spaces",
                true,
            ),
            (
                "Provide five Class B bicycle spaces.",
                "Five Class B bicycle spaces must be provided.",
                "Five Class B bicycle spaces",
                false,
            ),
            (
                "No amplified music is permitted on the patio.",
                "No amplified sound is permitted on the patio.",
                "No amplified sound is permitted on the patio.",
                false,
            ),
            (
                "No amplified music is permitted on the patio.",
                "No amplified music is permitted on the patio.",
                "No amplified music is permitted on the patio.",
                true,
            ),
            (
                "The owner pays for necessary or incidental street works.",
                "The owner pays for street works.",
                "The owner pays for street works.",
                true,
            ),
            (
                "Provide new or replacement duct banks.",
                "Provide duct banks.",
                "Provide duct banks.",
                true,
            ),
            (
                "Provide new or replacement duct banks.",
                "Provide new duct banks.",
                "Provide new duct banks.",
                false,
            ),
            (
                "Arrangements must be made before the permit for approval of the conversion.",
                "Approval must be completed before the permit.",
                "Approval must be completed before the permit.",
                false,
            ),
            (
                "Arrangements must be made before the permit for approval of the conversion.",
                "Arrange conversion approval before the permit.",
                "Arrange conversion approval before the permit.",
                true,
            ),
            (
                "Consolidate the parcels before a building permit to relocate the house.",
                "Consolidate the parcels before building permits.",
                "Consolidate the parcels before building permits.",
                false,
            ),
            (
                "Consolidate the parcels before a building permit to relocate the house.",
                "Consolidate the parcels before the building relocation permit.",
                "Consolidate the parcels before the building relocation permit.",
                true,
            ),
            (
                "Provide five Class B bicycle spaces.",
                "Conditions require five Class B bicycle spaces.",
                "five Class B bicycle spaces",
                true,
            ),
        ] {
            let mut response = sample_responses(true)[3].clone();
            response["claims"][0]["text"] = json!(claim);
            response["claims"][0]["quotes"][0]["text"] = json!(source);
            let verification: Verification = serde_json::from_value(response).unwrap();
            assert_eq!(
                validate_claims(&verification, body, &[source.into()], &[1]).is_ok(),
                expected,
                "{body}"
            );
        }
    }

    #[test]
    fn json_mode_tolerates_extra_metadata_but_requires_safety_fields() {
        let mut brief = sample_responses(true)[0].clone();
        brief["extra_note"] = json!("harmless metadata");
        brief["candidates"][0]
            .as_object_mut()
            .unwrap()
            .remove("editorial_reason");
        assert!(serde_json::from_value::<Brief>(brief.clone()).is_ok());
        for field in [
            "has_unresolved_conflict",
            "reports_advisory_method",
            "source_ids",
            "qualifications",
        ] {
            let mut missing = brief.clone();
            missing["candidates"][0]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(serde_json::from_value::<Brief>(missing).is_err(), "{field}");
        }
        for field in [
            "factual",
            "qualified",
            "newsworthy",
            "readable",
            "reports_advisory_method",
            "source_ids",
        ] {
            let mut review = sample_responses(true)[2].clone();
            review["reviews"][0].as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<Review>(review).is_err(), "{field}");
        }
    }

    #[test]
    fn code_labels_cannot_hide_or_replace_a_named_physical_requirement() {
        let pages = vec!["Upgrade to F4 and S4. The building must be sprinklered.".into()];
        let spans = source_spans(&pages);
        let mut brief: Brief = serde_json::from_value(sample_responses(true)[0].clone()).unwrap();
        brief.candidates[0].fact = "Upgrade to F4 and S4 and sprinkler the building.".into();
        let mut draft = Draft {
            text: "Building review calls for general fire and safety upgrades.".into(),
            candidate_indices: vec![0],
        };
        assert!(materialize(&draft, &brief, &spans, &pages, "https://example.com").is_err());
        draft.text = "Building review calls for sprinklers throughout the building.".into();
        assert!(materialize(&draft, &brief, &spans, &pages, "https://example.com").is_ok());
        assert!(weak_daycare_administration(
            "Test daycare",
            "Hours of operation: Monday to Friday."
        ));
        assert!(!weak_daycare_administration(
            "Test restaurant",
            "Patio hours of operation: Monday to Friday."
        ));
        assert!(weak_daycare_administration(
            "Test daycare",
            "The 20-child capacity will be determined at final licensing inspection."
        ));
        assert!(!weak_daycare_administration(
            "Test daycare",
            "Provide a two-hour fire separation to protect the outdoor play area."
        ));
    }

    #[test]
    fn opaque_cross_references_cannot_replace_a_concrete_condition() {
        let pages = vec!["Arrange legal agreements under section 2.3.1. Provide public life use of the private plaza.".into()];
        let spans = source_spans(&pages);
        let brief: Brief = serde_json::from_value(sample_responses(true)[0].clone()).unwrap();
        for text in [
            "Enter agreements under section 2.3.1 of the bulletin.",
            "Provide public life use of the space.",
            "The building must meet the code's minimum upgrade levels.",
            "Bring the building to 2025 design levels.",
            "Building review calls for full level 4 categories and sprinklers.",
            "Upgrade to level-4 and add sprinklers.",
            "Approval conditions: Test daycare. Provide five bicycle spaces.",
            "1 Test St daycare. Provide five bicycle spaces.",
            "Building review requires sprinklers installed at permit stage.",
            "At building permit application stage the house must be sprinklered.",
            "Consolidate the parcels before any building permit.",
            "Provide security before every permit.",
        ] {
            let draft = Draft {
                text: text.into(),
                candidate_indices: vec![0],
            };
            assert!(materialize(&draft, &brief, &spans, &pages, "https://example.com").is_err());
        }
        let draft = Draft {
            text: "Conditions call for public access rights over the private plaza.".into(),
            candidate_indices: vec![0],
        };
        assert!(materialize(&draft, &brief, &spans, &pages, "https://example.com").is_ok());
    }

    #[tokio::test]
    async fn malformed_review_retries_only_that_stage() {
        let mut responses = sample_responses(true);
        let mut incomplete = responses[2].clone();
        incomplete["reviews"][0]
            .as_object_mut()
            .unwrap()
            .remove("newsworthy");
        responses.insert(2, incomplete);
        let (client, server) = mock_client(responses).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &["Provide five Class B bicycle spaces in revised plans.".into()],
        )
        .await;
        assert!(result.summary.is_some(), "{:?}", result.error);
        assert_eq!(result.trace.len(), 5);
        assert!(result.trace[2]
            .error
            .as_ref()
            .unwrap()
            .contains("newsworthy"));
        let requests = server.await.unwrap();
        assert!(requests[3]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with(REVIEW));
        let repair: Value =
            serde_json::from_str(requests[3]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert!(repair["format_feedback"]
            .as_str()
            .unwrap()
            .contains("review_conditions"));
        assert_eq!(
            result
                .trace
                .iter()
                .filter(|s| s.stage == "select_conditions")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn opaque_lead_uses_next_eligible_ranked_fact_and_records_decision() {
        let mut responses = sample_responses(true);
        let mut opaque = responses[0]["candidates"][0].clone();
        opaque["fact"] = json!("Upgrade the building to F4, S4 and N4 design levels.");
        responses[0]["candidates"]
            .as_array_mut()
            .unwrap()
            .insert(0, opaque);
        responses[1]["drafts"][0]["candidate_indices"] = json!([1]);
        let (client, server) = mock_client(responses).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &[
                "Upgrade to F4, S4 and N4. Provide five Class B bicycle spaces in revised plans."
                    .into(),
            ],
        )
        .await;
        assert!(result.summary.is_some(), "{:?}", result.error);
        assert_eq!(result.trace.len(), 4);
        assert!(result.trace[0]
            .decision
            .as_ref()
            .unwrap()
            .contains("candidate 1"));
        let requests = server.await.unwrap();
        let writer: Value =
            serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(writer["selected_candidate_index"], 1);
    }

    #[tokio::test]
    async fn occupancy_labels_do_not_discard_a_concrete_separation_requirement() {
        let mut responses = sample_responses(true);
        responses[0]["candidates"][0]["fact"] =
            json!("Require 2-hour separation between the A2 daycare space and F2 garbage room.");
        let text = "Provide 2-hour separation between the daycare space and garbage room.";
        responses[1]["drafts"][0]["text"] = json!(text);
        responses[3]["claims"][0]["text"] = json!(text);
        responses[3]["claims"][0]["quotes"][0]["text"] = json!(text);
        let (client, server) = mock_client(responses).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &[text.into()],
        )
        .await;
        assert!(result.summary.is_some(), "{:?}", result.error);
        assert!(result.trace[0].decision.is_none());
        let requests = server.await.unwrap();
        let writer: Value =
            serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(writer["selected_candidate_index"], 0);
        assert_eq!(writer["required_concrete_terms"], json!(["separation"]));
    }

    #[tokio::test]
    async fn generic_but_accurate_posts_cannot_bypass_final_audit() {
        let mut round = sample_responses(true);
        round[3]["concrete"] = json!(false);
        round[3]["issues"] = json!(["Only generic compliance language."]);
        let (client, server) =
            mock_client((0..MAX_ROUNDS).flat_map(|_| round.clone()).collect()).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &["Provide five Class B bicycle spaces in revised plans.".into()],
        )
        .await;
        assert!(result.summary.is_none());
        assert!(result.error.unwrap().contains("generic compliance"));
        assert_eq!(server.await.unwrap().len(), 4 * MAX_ROUNDS);
    }

    #[tokio::test]
    async fn rejected_first_choice_reuses_other_drafts_with_a_fresh_review_and_audit() {
        let good = sample_responses(true);
        let mut responses = good.clone();
        responses[1]["drafts"]
            .as_array_mut()
            .unwrap()
            .push(good[1]["drafts"][0].clone());
        responses[1]["drafts"][0]["text"] = json!("Provide six Class B bicycle spaces.");
        responses[3]["accurate"] = json!(false);
        responses[3]["issues"] = json!(["The source requires five spaces, not six."]);
        let mut next_review = good[2].clone();
        next_review["best_index"] = json!(1);
        next_review["reviews"][0]["index"] = json!(1);
        responses.extend([next_review, good[3].clone()]);
        let (client, server) = mock_client(responses).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &["Provide five Class B bicycle spaces in revised plans.".into()],
        )
        .await;
        assert!(result.summary.unwrap().overview.contains("five"));
        assert_eq!(result.trace.len(), 6);
        assert!(result.trace.iter().all(|step| step.round == 1));
        assert!(result.trace[3].error.as_ref().unwrap().contains("not six"));
        let requests = server.await.unwrap();
        let second_review: Value =
            serde_json::from_str(requests[4]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(second_review["drafts"].as_array().unwrap().len(), 1);
        assert_eq!(second_review["drafts"][0]["index"], 1);
        let audit: Value =
            serde_json::from_str(requests[5]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(audit.as_object().unwrap().len(), 3);
        assert!(!audit.to_string().contains("six"));
    }

    #[test]
    fn provider_rejections_stop_alternative_drafts_and_outer_rounds() {
        for message in [
            "credit_balance_exhausted",
            "insufficient_quota",
            "402 Payment Required",
            "401 Unauthorized",
        ] {
            assert!(provider_rejected(message));
        }
        assert!(!provider_rejected("Final source audit failed"));
    }

    #[test]
    fn separate_claims_can_cover_both_sides_of_plus() {
        let page = "Provide a structural assessment and a seismic retrofit design.";
        let verification: Verification = serde_json::from_value(json!({
            "claims": [
                {"text":"Provide a structural assessment", "quotes":[{"page":1,"text":page}], "entailed":true,"explanation":"Required report."},
                {"text":"a seismic retrofit design", "quotes":[{"page":1,"text":page}], "entailed":true,"explanation":"Required design."}
            ], "meaning_changes":[], "assessment":"Both required deliverables are supported.",
            "accurate":true,"qualified":true,"concrete":true,"issues":[]
        })).unwrap();
        assert!(validate_claims(
            &verification,
            "Provide a structural assessment plus a seismic retrofit design.",
            &[page.into()],
            &[1]
        )
        .is_ok());
        assert!(validate_claims(
            &verification,
            "Provide a structural assessment plus a completed seismic retrofit design.",
            &[page.into()],
            &[1]
        )
        .is_err());
    }

    #[tokio::test]
    async fn final_audit_can_overrule_the_editor_without_seeing_its_verdict() {
        let mut responses = sample_responses(true);
        responses[3] = json!({"claims":[],"meaning_changes":[],"assessment":"A part of the named parcel is not the whole parcel.", "accurate":false,"qualified":false,"concrete":true,"issues":["Preserve the parcel's limited scope."]});
        responses.extend(sample_responses(true));
        let (client, server) = mock_client(responses).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &["Provide five Class B bicycle spaces in revised plans.".into()],
        )
        .await;
        assert!(result.summary.is_some(), "{:?}", result.error);
        assert_eq!(result.trace.len(), 8);
        assert!(result.trace[3]
            .error
            .as_ref()
            .unwrap()
            .contains("limited scope"));
        let requests = server.await.unwrap();
        let audit: Value =
            serde_json::from_str(requests[3]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(audit.as_object().unwrap().len(), 3);
        assert_eq!(audit["source_pages"][0]["page"], 1);
        assert!(audit["post"].as_str().unwrap().contains("five"));
        let repair: Value =
            serde_json::from_str(requests[4]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert!(repair["previous_feedback"]
            .as_str()
            .unwrap()
            .contains("limited scope"));
    }

    #[tokio::test]
    async fn acknowledged_meaning_changes_fail_even_when_all_booleans_pass() {
        let mut responses = sample_responses(true);
        responses[3]["meaning_changes"] = json!(["Possible hazard became a definite hazard."]);
        responses.extend(sample_responses(true));
        let (client, server) = mock_client(responses).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &["Provide five Class B bicycle spaces in revised plans.".into()],
        )
        .await;
        assert!(result.summary.is_some(), "{:?}", result.error);
        assert_eq!(result.trace.len(), 8);
        assert!(result.trace[3]
            .error
            .as_ref()
            .unwrap()
            .contains("definite hazard"));
        assert_eq!(server.await.unwrap().len(), 8);
    }

    #[tokio::test]
    async fn long_drafts_repair_using_measured_lengths_without_reselecting() {
        let mut responses = sample_responses(true);
        responses.insert(1, json!({"drafts":[{"text":format!("{}.", "Excess detail ".repeat(40)),"candidate_indices":[0]}]}));
        let (client, server) = mock_client(responses).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &["Provide five Class B bicycle spaces in revised plans.".into()],
        )
        .await;
        assert!(result.summary.is_some(), "{:?}", result.error);
        assert_eq!(result.trace.len(), 5);
        assert!(result.trace.iter().all(|s| s.round == 1));
        assert!(result.trace[1]
            .error
            .as_ref()
            .unwrap()
            .contains("over budget"));
        let requests = server.await.unwrap();
        let repair: Value =
            serde_json::from_str(requests[2]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert!(repair["previous_feedback"]
            .as_str()
            .unwrap()
            .contains("Measured validation results"));
        assert!(repair["target_words"].as_u64().unwrap() <= 16);
        assert!(requests[2]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with(WRITE));
    }

    #[tokio::test]
    async fn workflow_repairs_with_feedback_and_resolves_evidence_without_model_quotes() {
        let mut responses = sample_responses(false);
        responses.extend(sample_responses(true));
        let (client, server) = mock_client(responses).await;
        let pages = vec!["Conditional approval for 1 Test St daycare. Revised drawings must provide five Class B bicycle spaces.".into()];
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "1 Test St daycare",
            "https://example.com/letter",
            &pages,
        )
        .await;
        let summary = result.summary.unwrap();
        assert_eq!(summary.requirements[0].evidence, pages[0]);
        assert_eq!(
            summary.overview,
            "Approval conditions: 1 Test St daycare. Provide five Class B bicycle spaces."
        );
        assert_eq!(result.trace.len(), 7);
        assert!(result.trace[2].error.is_some());
        let requests = server.await.unwrap();
        let repair: Value =
            serde_json::from_str(requests[3]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert!(repair["previous_feedback"]
            .as_str()
            .unwrap()
            .contains("Timing needs correction"));
        assert!(requests[0]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with(SELECT));
        assert!(requests[1]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with(WRITE));
        assert!(requests[2]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with(REVIEW));
    }

    #[tokio::test]
    async fn unknown_source_ids_fail_closed_after_bounded_rounds() {
        let invalid = json!({"project_label":"Test", "lead_candidate_index":0,"lead_rationale":"", "candidates":[{"fact":"Invented fee", "has_unresolved_conflict":false,"reports_advisory_method":false,"source_ids":["nonexistent"], "stage":"", "qualifications":"", "editorial_reason":""}]});
        let (client, server) = mock_client(vec![invalid; MAX_ROUNDS]).await;
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &["A complete source paragraph without any invented fee.".into()],
        )
        .await;
        assert!(result.summary.is_none());
        assert!(result.error.unwrap().contains("Unknown source ID"));
        assert_eq!(result.trace.len(), MAX_ROUNDS);
        assert_eq!(server.await.unwrap().len(), MAX_ROUNDS);
    }

    #[tokio::test]
    async fn reviewer_can_correct_a_citation_without_rewriting_supported_prose() {
        let mut responses = sample_responses(true);
        responses[2]["reviews"][0]["source_ids"] = json!(["p2s1"]);
        responses[3]["claims"][0]["quotes"][0]["page"] = json!(2);
        let (client, server) = mock_client(responses).await;
        let pages = vec![
            "Introduction to the letter and the proposed site changes.".into(),
            "Revised drawings must provide five Class B bicycle spaces.".into(),
        ];
        let result = analyze_pages(
            &client,
            DEFAULT_MODEL,
            "Test",
            "https://example.com",
            &pages,
        )
        .await;
        let summary = result.summary.unwrap();
        assert_eq!(summary.requirements[0].page, 2);
        assert_eq!(summary.requirements[0].evidence, pages[1]);
        assert_eq!(result.trace.len(), 4);
        let requests = server.await.unwrap();
        let audit: Value =
            serde_json::from_str(requests[3]["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(audit["source_pages"].as_array().unwrap().len(), 2);
        assert_eq!(audit["source_pages"][1]["text"], pages[1]);
    }

    #[test]
    fn reviewer_cannot_invent_or_omit_citations() {
        let pages = vec!["Provide five Class B bicycle spaces in the revised drawings.".into()];
        let spans = source_spans(&pages);
        let brief: Brief = serde_json::from_value(sample_responses(true)[0].clone()).unwrap();
        let draft = Draft {
            text: "Provide five Class B bicycle spaces.".into(),
            candidate_indices: vec![0],
        };
        let mut summary =
            materialize(&draft, &brief, &spans, &pages, "https://example.com").unwrap();
        let mut review: Review = serde_json::from_value(sample_responses(true)[2].clone()).unwrap();
        for ids in [
            vec![],
            vec!["missing".into()],
            vec!["p1s1".into(), "p1s1".into()],
        ] {
            review.reviews[0].source_ids = ids;
            assert!(
                reviewed_evidence(&mut summary, &review.reviews[0], &draft, &spans, &pages)
                    .is_err()
            );
        }
    }

    #[test]
    fn citations_keep_full_page_context_and_deduplicate_pages() {
        let page = format!("{}\n\nThe sidewalk must be rebuilt, subject to the following exception.\n\nThe exception permits retaining the existing curb.", "Public realm introduction. ".repeat(45));
        let pages = vec![page];
        let spans = source_spans(&pages);
        assert!(spans.len() > 1);
        let brief: Brief = serde_json::from_value(sample_responses(true)[0].clone()).unwrap();
        let draft = Draft {
            text: "Plans must show sidewalk reconstruction.".into(),
            candidate_indices: vec![0],
        };
        let mut summary =
            materialize(&draft, &brief, &spans, &pages, "https://example.com").unwrap();
        let mut review: Review = serde_json::from_value(sample_responses(true)[2].clone()).unwrap();
        review.reviews[0].source_ids = spans.iter().map(|s| s.id.clone()).collect();
        reviewed_evidence(&mut summary, &review.reviews[0], &draft, &spans, &pages).unwrap();
        assert_eq!(summary.requirements.len(), 1);
        assert_eq!(summary.requirements[0].evidence, normalized(&pages[0]));
        assert!(summary.requirements[0]
            .evidence
            .contains("retaining the existing curb"));
    }

    #[test]
    fn source_ids_preserve_page_numbers_and_resolve_exact_text() {
        let pages = vec![
            "A condition with  fancy\n spacing.\n\nAnother condition.".into(),
            "A second page.".into(),
        ];
        let spans = source_spans(&pages);
        assert_eq!(spans[0].id, "p1s1");
        assert_eq!(spans.last().unwrap().page, 2);
        for span in spans {
            assert!(normalized(&pages[span.page - 1]).contains(&span.text));
        }
    }

    #[test]
    fn review_cannot_override_failed_checks_or_omit_the_chosen_draft() {
        let mut review = Review {
            _assessment: "Test verdict".into(),
            best_index: 0,
            reviews: vec![Verdict {
                index: 0,
                source_ids: vec!["p1s1".into()],
                reports_advisory_method: false,
                factual: true,
                qualified: false,
                newsworthy: true,
                readable: true,
                issues: vec![],
                _suggestions: vec![],
            }],
            repair_feedback: String::new(),
        };
        assert!(accepted_index(&review, &[0]).is_err());
        review.reviews[0].qualified = true;
        assert_eq!(accepted_index(&review, &[0]).unwrap(), 0);
        assert_eq!(accepted_index(&review, &[0, 1]).unwrap(), 0);
        review.reviews[0].newsworthy = false;
        assert!(accepted_index(&review, &[0]).is_err());
        review.reviews[0].newsworthy = true;
        review.best_index = 1;
        assert!(accepted_index(&review, &[0]).is_err());
        assert!(accepted_index(&review, &[0, 1]).is_err());
        let mut extra: Verdict =
            serde_json::from_value(sample_responses(true)[2]["reviews"][0].clone()).unwrap();
        extra.index = 1;
        review.reviews.push(extra);
        review.best_index = 0;
        assert_eq!(accepted_index(&review, &[0, 1]).unwrap(), 0);
        review.reviews[1].index = 0;
        assert!(
            accepted_index(&review, &[0, 1]).is_err(),
            "Conflicting duplicate verdicts must fail"
        );
    }

    #[test]
    fn jargon_and_lost_lead_fail_before_review() {
        let brief: Brief = serde_json::from_value(sample_responses(true)[0].clone()).unwrap();
        let pages = vec!["The building must meet F4 and N4 upgrade levels and provide five Class B bicycle spaces.".into()];
        let spans = source_spans(&pages);
        for draft in [
            Draft {
                text: "Building review calls for F4 and N4 upgrades.".into(),
                candidate_indices: vec![0],
            },
            Draft {
                text: "Provide five bicycle spaces.".into(),
                candidate_indices: vec![1],
            },
        ] {
            assert!(materialize(&draft, &brief, &spans, &pages, "https://example.com").is_err());
        }
    }

    #[test]
    fn acknowledged_conflict_cannot_be_selected_or_added_to_a_post() {
        let mut brief: Brief = serde_json::from_value(sample_responses(true)[0].clone()).unwrap();
        let pages = vec![
            "Remove the parking. A separate condition allows retaining it after analysis.".into(),
        ];
        let spans = source_spans(&pages);
        brief.candidates[0].has_unresolved_conflict = true;
        assert!(validate_brief(&brief, &spans).is_err());
        let draft = Draft {
            text: "Retain the parking after analysis.".into(),
            candidate_indices: vec![0],
        };
        assert!(materialize(&draft, &brief, &spans, &pages, "https://example.com").is_err());
    }

    #[test]
    fn advisory_design_paths_cannot_be_promoted_to_mandatory_changes() {
        assert!(validate_advisory_wording("Plans must enlarge the windows.", true).is_err());
        assert!(validate_advisory_wording(
            "The letter suggests enlarging windows to improve livability.",
            true
        )
        .is_ok());
        let pages = vec!["Revised drawings are required. Improve livability; this can be achieved by enlarging windows. Separately, protect six neighbouring trees.".into()];
        let spans = source_spans(&pages);
        let mut brief: Brief = serde_json::from_value(sample_responses(true)[0].clone()).unwrap();
        let draft = Draft {
            text: "Plans must protect six neighbouring trees.".into(),
            candidate_indices: vec![0],
        };
        let mut summary =
            materialize(&draft, &brief, &spans, &pages, "https://example.com").unwrap();
        let mut review: Review = serde_json::from_value(sample_responses(true)[2].clone()).unwrap();
        // Unrelated advice sharing a citation must not reject a genuine requirement.
        reviewed_evidence(&mut summary, &review.reviews[0], &draft, &spans, &pages).unwrap();
        // Either source-reading stage can identify advice and enforce explicit wording.
        brief.candidates[0].reports_advisory_method = true;
        assert!(materialize(&draft, &brief, &spans, &pages, "https://example.com").is_err());
        review.reviews[0].reports_advisory_method = true;
        assert!(
            reviewed_evidence(&mut summary, &review.reviews[0], &draft, &spans, &pages).is_err()
        );
    }
}
