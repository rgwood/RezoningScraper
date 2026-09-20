//! Passive approval history. Nothing in this module enqueues or publishes posts.
use anyhow::{bail, Context, Result};
use chrono::{NaiveDate, Utc};
use regex::Regex;
use reqwest::{Client, Url};
use rusqlite::{params, Connection, OptionalExtension};
use scraper::{Html, Selector};
use serde::Serialize;
use std::{sync::LazyLock, time::Duration};

use crate::{db::Database, models::Project};

const RECHECK_SECONDS: i64 = 24 * 60 * 60;
const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;

pub fn initialize_schema(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS ApprovalScans (
            ProjectId TEXT PRIMARY KEY,
            FirstSeen INTEGER NOT NULL,
            SourceText TEXT NOT NULL,
            LastSuccess INTEGER
        );
        CREATE TABLE IF NOT EXISTS ApprovalEvents (
            Id INTEGER PRIMARY KEY,
            ProjectId TEXT NOT NULL,
            ProjectName TEXT NOT NULL,
            ApplicationNumber TEXT,
            ProjectUrl TEXT NOT NULL,
            Kind TEXT NOT NULL,
            DecisionDate TEXT NOT NULL,
            Authority TEXT,
            Notice TEXT NOT NULL,
            FirstSeen INTEGER NOT NULL,
            LastSeen INTEGER NOT NULL,
            IsBaseline INTEGER NOT NULL,
            UNIQUE(ProjectId, Kind, DecisionDate, Notice)
        );
        CREATE TABLE IF NOT EXISTS ApprovalDocuments (
            Id INTEGER PRIMARY KEY,
            ProjectId TEXT NOT NULL,
            SourceUrl TEXT NOT NULL,
            Title TEXT NOT NULL,
            FirstSeen INTEGER NOT NULL,
            LastSeen INTEGER NOT NULL,
            LastChecked INTEGER,
            LastError TEXT,
            UNIQUE(ProjectId, SourceUrl)
        );
        CREATE TABLE IF NOT EXISTS ApprovalDocumentVersions (
            Id INTEGER PRIMARY KEY,
            DocumentId INTEGER NOT NULL REFERENCES ApprovalDocuments(Id),
            DownloadUrl TEXT NOT NULL,
            FirstSeen INTEGER NOT NULL,
            LastSeen INTEGER NOT NULL,
            Content BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS ApprovalDocumentVersionsByDocument
            ON ApprovalDocumentVersions(DocumentId);",
    )?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub kind: String,
    /// The date stated by the City, not the date we first saw the notice.
    pub date: Option<String>,
    pub authority: Option<String>,
    pub notice: String,
}

static DEVELOPMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:the )?(Director of Planning|Development Permit Board) approved (?:this|the) application\b").unwrap()
});
static REZONING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^this application was approved by (?:city )?council\b").unwrap()
});
static PERMIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:a )?development permit was issued on\b").unwrap());
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(January|February|March|April|May|June|July|August|September|October|November|December)\s+(\d{1,2})(?:st|nd|rd|th)?\s*,?\s+(\d{4})\b").unwrap()
});
static APPLICATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bDP-\d{4}-\d+\b").unwrap());

fn normalized_text(html: &str) -> String {
    Html::parse_fragment(html)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn date_in(text: &str) -> Option<String> {
    let captures = DATE.captures(text)?;
    let value = format!("{} {} {}", &captures[1], &captures[2], &captures[3]);
    NaiveDate::parse_from_str(&value, "%B %d %Y")
        .ok()
        .map(|date| date.to_string())
}

pub fn decisions(project: &Project) -> Vec<Decision> {
    let name = project.attributes.name.to_lowercase();
    let development = name.contains("development application");
    let rezoning = name.contains("rezoning application");
    if !development && !rezoning {
        return Vec::new();
    }
    let mut found = Vec::new();
    for source in [
        &project.attributes.archival_reason_message,
        &project.attributes.description,
    ]
    .into_iter()
    .flatten()
    {
        let html = Html::parse_fragment(source);
        let selector = Selector::parse("p").unwrap();
        let paragraphs: Vec<_> = html.select(&selector).map(|p| p.inner_html()).collect();
        let paragraphs = if paragraphs.is_empty() {
            vec![source.clone()]
        } else {
            paragraphs
        };
        for paragraph in paragraphs {
            let notice = normalized_text(&paragraph);
            let decision = if development {
                let approval = DEVELOPMENT.captures(&notice);
                let issued = PERMIT.find(&notice);
                // An issuance sentence may stand alone, or follow an explicit approval.
                if let Some(issued) = issued.filter(|m| m.start() == 0 || approval.is_some()) {
                    // Some notices give both dates. Keep the earlier approval, but
                    // never use the permit's date as an undated approval's date.
                    if let Some(approval) = &approval {
                        let approval_text = &notice[..issued.start()];
                        if let Some(date) = date_in(approval_text) {
                            let decision = development_decision(
                                approval_text,
                                &approval[1],
                                Some(date),
                                &notice,
                            );
                            if !found.contains(&decision) {
                                found.push(decision);
                            }
                        }
                    }
                    Some(Decision {
                        kind: "permit_issued".into(),
                        date: date_in(&notice[issued.end()..]),
                        authority: approval.map(|c| c[1].to_string()),
                        notice: notice.clone(),
                    })
                } else {
                    approval.map(|c| {
                        development_decision(
                            &notice,
                            &c[1],
                            date_in(notice.split('.').next().unwrap_or(&notice)),
                            &notice,
                        )
                    })
                }
            } else if REZONING.is_match(&notice) {
                Some(Decision {
                    kind: "rezoning_approved".into(),
                    date: date_in(notice.split('.').next().unwrap_or(&notice)),
                    authority: Some("Council".into()),
                    notice: notice.clone(),
                })
            } else {
                None
            };
            if let Some(decision) = decision {
                if !found.contains(&decision) {
                    found.push(decision);
                }
            }
        }
    }
    found
}

fn development_decision(
    text: &str,
    authority: &str,
    date: Option<String>,
    notice: &str,
) -> Decision {
    Decision {
        kind: if text.to_lowercase().contains("subject to conditions") {
            "development_approved_with_conditions"
        } else {
            "development_approved"
        }
        .into(),
        date,
        authority: Some(authority.into()),
        notice: notice.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentLink {
    pub title: String,
    pub url: Url,
}

pub fn document_links(page: &str, base: &Url) -> Vec<DocumentLink> {
    let html = Html::parse_document(page);
    // Q&A answers sometimes link a neighbouring project's conditions. Restrict
    // discovery to the City's document library and project description.
    let selector =
        Selector::parse(".widget_document_library a[href], .description a[href]").unwrap();
    let mut links = Vec::new();
    for anchor in html.select(&selector) {
        let title = normalized_text(&anchor.inner_html());
        let label = title.to_lowercase().replace(['‐', '‑', '–'], "-");
        if !(label.contains("prior-to letter")
            || label.contains("prior to letter")
            || label.contains("conditions of approval"))
        {
            continue;
        }
        if let Ok(mut url) = base.join(anchor.value().attr("href").unwrap()) {
            if !matches!(url.scheme(), "https" | "http") || !url.username().is_empty() {
                continue;
            }
            url.set_fragment(None);
            if !links.iter().any(|link: &DocumentLink| link.url == url) {
                links.push(DocumentLink { title, url });
            }
        }
    }
    links
}

async fn get_bytes(client: &Client, url: &Url, limit: usize) -> Result<(Url, Vec<u8>)> {
    let mut response = client.get(url.clone()).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        bail!("Response exceeds {limit} bytes: {url}");
    }
    let final_url = response.url().clone();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > limit {
            bail!("Response exceeds {limit} bytes: {url}");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((final_url, bytes))
}

pub async fn download_pdf(client: &Client, url: &Url) -> Result<(Url, Vec<u8>)> {
    let (resolved, bytes) = get_bytes(client, url, MAX_PDF_BYTES).await?;
    if bytes.starts_with(b"%PDF-") {
        return Ok((resolved, bytes));
    }
    let wrapper = Html::parse_document(std::str::from_utf8(&bytes)?);
    let selector = Selector::parse("#documents-show-data[data-document-id]").unwrap();
    if wrapper.select(&selector).next().is_none() {
        bail!("Expected a PDF or Shape Your City download page: {resolved}");
    }
    // The site's document_show script navigates to pathname + '/download'.
    let mut download = resolved.clone();
    download.set_path(&format!(
        "{}/download",
        resolved.path().trim_end_matches('/')
    ));
    download.set_query(None);
    download.set_fragment(None);
    let (resolved, bytes) = get_bytes(client, &download, MAX_PDF_BYTES).await?;
    if !bytes.starts_with(b"%PDF-") {
        bail!("Download did not return a PDF: {resolved}");
    }
    Ok((resolved, bytes))
}

fn record_decisions(db: &Connection, project: &Project, now: i64) -> Result<Vec<Decision>> {
    let tx = db.unchecked_transaction()?;
    let baseline = tx.execute(
        "INSERT OR IGNORE INTO ApprovalScans(ProjectId, FirstSeen, SourceText)
         VALUES (?1, ?2, '')",
        params![project.id, now],
    )? == 1;
    let found = decisions(project);
    let application = APPLICATION
        .find(&project.attributes.name)
        .map(|m| m.as_str());
    for decision in &found {
        tx.execute(
            "INSERT INTO ApprovalEvents
             (ProjectId, ApplicationNumber, ProjectUrl, Kind, DecisionDate, Authority,
              Notice, FirstSeen, LastSeen, IsBaseline, ProjectName)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10)
             ON CONFLICT(ProjectId, Kind, DecisionDate, Notice)
             DO UPDATE SET LastSeen = excluded.LastSeen",
            params![
                project.id,
                application,
                project.links.self_link,
                decision.kind,
                decision.date.as_deref().unwrap_or(""),
                decision.authority,
                decision.notice,
                now,
                baseline,
                project.attributes.name,
            ],
        )?;
    }
    tx.commit()?;
    Ok(found)
}

fn record_document(
    db: &Connection,
    project_id: &str,
    link: &DocumentLink,
    now: i64,
) -> Result<i64> {
    db.execute(
        "INSERT INTO ApprovalDocuments(ProjectId, SourceUrl, Title, FirstSeen, LastSeen)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(ProjectId, SourceUrl) DO UPDATE SET
            Title = excluded.Title, LastSeen = excluded.LastSeen",
        params![project_id, link.url.as_str(), link.title, now],
    )?;
    Ok(db.query_row(
        "SELECT Id FROM ApprovalDocuments WHERE ProjectId = ?1 AND SourceUrl = ?2",
        params![project_id, link.url.as_str()],
        |row| row.get(0),
    )?)
}

fn record_pdf(db: &mut Database, id: i64, url: &Url, bytes: &[u8], now: i64) -> Result<()> {
    let tx = db.transaction()?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT Id FROM ApprovalDocumentVersions WHERE DocumentId = ?1 AND Content = ?2",
            params![id, bytes],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(version) = existing {
        tx.execute(
            "UPDATE ApprovalDocumentVersions SET LastSeen = ?2 WHERE Id = ?1",
            params![version, now],
        )?;
    } else {
        tx.execute(
            "INSERT INTO ApprovalDocumentVersions(DocumentId, DownloadUrl, FirstSeen, LastSeen, Content)
             VALUES (?1, ?2, ?3, ?3, ?4)", params![id, url.as_str(), now, bytes],
        )?;
    }
    tx.execute(
        "UPDATE ApprovalDocuments SET LastChecked = ?2, LastError = NULL WHERE Id = ?1",
        params![id, now],
    )?;
    tx.commit()?;
    Ok(())
}

/// Called before the posting pipeline. Fetch failures preserve evidence already collected
/// and return an error after other projects have been attempted.
pub async fn track_approvals(db: &mut Database, projects: &[Project]) -> Result<()> {
    let client = Client::builder().timeout(Duration::from_secs(45)).build()?;
    track_with_client(db, projects, &client, Utc::now().timestamp()).await
}

async fn track_with_client(
    db: &mut Database,
    projects: &[Project],
    client: &Client,
    now: i64,
) -> Result<()> {
    let mut failures = Vec::new();
    for project in projects {
        let name = project.attributes.name.to_lowercase();
        if !name.contains("development application") && !name.contains("rezoning application") {
            continue;
        }
        let found = record_decisions(db, project, now)?;
        let known: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM ApprovalEvents WHERE ProjectId = ?1)",
            [&project.id],
            |row| row.get(0),
        )?;
        if found.is_empty() && !known {
            continue;
        }
        let source = serde_json::to_string(&(
            &project.attributes.archival_reason_message,
            &project.attributes.description,
        ))?;
        let (previous, last_success): (String, Option<i64>) = db.query_row(
            "SELECT SourceText, LastSuccess FROM ApprovalScans WHERE ProjectId = ?1",
            [&project.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if previous == source && last_success.is_some_and(|last| now - last < RECHECK_SECONDS) {
            continue;
        }
        match collect_documents(db, project, client, now).await {
            Ok(()) => {
                db.execute("UPDATE ApprovalScans SET SourceText = ?2, LastSuccess = ?3 WHERE ProjectId = ?1",
                    params![project.id, source, now])?;
            }
            Err(error) => {
                let message = format!("{}: {error:#}", project.attributes.name);
                eprintln!("Approval tracking failed: {message}");
                failures.push(message);
            }
        }
    }
    if !failures.is_empty() {
        bail!(
            "Approval tracking failed for {} project(s): {}",
            failures.len(),
            failures.join("; ")
        );
    }
    Ok(())
}

async fn collect_documents(
    db: &mut Database,
    project: &Project,
    client: &Client,
    now: i64,
) -> Result<()> {
    let url = Url::parse(&project.links.self_link).context("Invalid project URL")?;
    let (url, page) = get_bytes(client, &url, MAX_PAGE_BYTES).await?;
    let links = document_links(std::str::from_utf8(&page)?, &url);
    let mut failures = Vec::new();
    for link in links {
        let id = record_document(db, &project.id, &link, now)?;
        match download_pdf(client, &link.url).await {
            Ok((url, bytes)) => record_pdf(db, id, &url, &bytes, now)?,
            Err(error) => {
                let message = format!("{error:#}");
                db.execute(
                    "UPDATE ApprovalDocuments SET LastChecked = ?2, LastError = ?3 WHERE Id = ?1",
                    params![id, now, message],
                )?;
                failures.push(format!("{}: {message}", link.url));
            }
        }
    }
    if !failures.is_empty() {
        bail!("{}", failures.join("; "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Projects;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    const CONDITIONAL: &str = "The Director of Planning approved the application on May 7, 2026, subject to conditions. A Development Permit may be issued once all conditions have been satisfied.";
    const ISSUED: &str = "The Director of Planning approved this application, and a Development Permit was issued on July 24, 2026";

    fn project(notice: &str) -> Project {
        Project {
            id: "52066".into(),
            project_type: "projects".into(),
            attributes: crate::models::Attributes {
                name: "4615 Arbutus St (DP-2026-00114) development application".into(),
                archival_reason_message: Some(notice.into()),
                ..Default::default()
            },
            relationships: Default::default(),
            links: crate::models::Links1 {
                self_link: "https://www.shapeyourcity.ca/4615-arbutus-st-2".into(),
            },
        }
    }

    fn count(db: &Connection, table: &str) -> i64 {
        db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn detects_conditional_approval_from_real_page_excerpt() {
        let mut p = project("");
        p.attributes.description = Some(include_str!("../test_files/approval-arbutus.html").into());
        let found = decisions(&p);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "development_approved_with_conditions");
        assert_eq!(found[0].date.as_deref(), Some("2026-05-07"));
        assert_eq!(found[0].authority.as_deref(), Some("Director of Planning"));
    }

    #[test]
    fn detects_issuance_and_board_decisions() {
        for authority in ["Director of Planning", "Development Permit Board"] {
            let found = decisions(&project(&ISSUED.replace("Director of Planning", authority)));
            assert_eq!(found[0].kind, "permit_issued");
            assert_eq!(found[0].date.as_deref(), Some("2026-07-24"));
            assert_eq!(found[0].authority.as_deref(), Some(authority));
        }
        assert_eq!(
            decisions(&project("A Development Permit was issued on July 24, 2026"))[0].kind,
            "permit_issued"
        );
    }

    #[test]
    fn combined_notice_preserves_both_decision_dates() {
        let found = decisions(&project("The Director of Planning approved the application on May 7, 2026, subject to conditions. A Development Permit was issued on July 24, 2026."));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, "development_approved_with_conditions");
        assert_eq!(found[0].date.as_deref(), Some("2026-05-07"));
        assert_eq!(found[1].kind, "permit_issued");
        assert_eq!(found[1].date.as_deref(), Some("2026-07-24"));
    }

    #[test]
    fn unrelated_later_date_is_not_used_as_the_approval_date() {
        let found = decisions(&project("The Director of Planning approved this application, subject to conditions. The next meeting is on July 24, 2026."));
        assert_eq!(found[0].date, None);
    }

    #[test]
    fn rejects_background_approvals_and_undecided_or_withdrawn_applications() {
        for notice in [
            "Consultation has concluded.",
            "This application has been withdrawn.",
            "The Director of Planning did not approve this application.",
            "The Director of Planning may approve this application, subject to conditions.",
            "Under the site's zoning the application is conditional and requires the decision of the Director of Planning.",
            "This development application follows the rezoning application approved in principle by City Council on January 13, 2026.",
            "This application was approved by Council at Public Hearing on January 13, 2026.",
            "If the Director of Planning approved the application on May 7, 2026, it could proceed.",
            "A Development Permit may be issued once all conditions have been satisfied.",
        ] {
            assert!(decisions(&project(notice)).is_empty(), "false positive: {notice}");
        }
        let mut other = project(CONDITIONAL);
        other.attributes.name = "Citywide policy consultation".into();
        assert!(decisions(&other).is_empty());
    }

    #[test]
    fn distinguishes_rezoning_and_parses_ordinal_dates() {
        let mut p = project("<p><strong>This application was approved by Council at Public Hearing on March 11th, 2021.</strong></p>");
        p.attributes.name = "A rezoning application".into();
        let found = decisions(&p);
        assert_eq!(found[0].kind, "rezoning_approved");
        assert_eq!(found[0].date.as_deref(), Some("2021-03-11"));
    }

    #[test]
    fn missing_or_invalid_dates_remain_unknown() {
        for date in ["", "February 31, 2026"] {
            let p = project(&format!("The Director of Planning approved the application on {date}, subject to conditions."));
            let found = decisions(&p);
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].date, None);
        }
    }

    #[test]
    fn deduplicates_html_and_whitespace_variants() {
        let mut p = project(&format!("<p><strong>{CONDITIONAL}</strong></p>"));
        p.attributes.description = Some(CONDITIONAL.replace(" ", " &nbsp; "));
        assert_eq!(decisions(&p).len(), 1);
    }

    #[test]
    fn existing_api_fixture_contains_a_real_issued_permit() {
        let projects: Projects =
            serde_json::from_str(include_str!("../test_files/ExampleInput.json")).unwrap();
        let p = projects
            .data
            .iter()
            .find(|p| p.attributes.name.starts_with("524-528 Powell"))
            .unwrap();
        let found = decisions(p);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "permit_issued");
        assert_eq!(found[0].date.as_deref(), Some("2021-09-20"));
    }

    #[test]
    fn finds_real_conditions_link_and_ignores_unrelated_documents() {
        let base = Url::parse("https://www.shapeyourcity.ca/4615-arbutus-st-2").unwrap();
        let links = document_links(include_str!("../test_files/approval-arbutus.html"), &base);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].url.as_str(),
            "https://www.shapeyourcity.ca/52066/widgets/220230/documents/168565"
        );
    }

    #[test]
    fn resolves_and_deduplicates_links_without_executing_javascript() {
        let page = r#"<a href="/letter#page=1">Prior-to letter</a>
            <a href="/letter">Prior to letter</a>
            <a href="other.pdf?a=1&amp;b=2">Conditions of approval</a>
            <a href="javascript:alert(1)">Prior-to letter</a>
            <a href="file:///tmp/secret">Prior-to letter</a>
            <a href="https://user:pass@example.com/">Prior-to letter</a>
            <a href="plans.pdf">Application drawings</a>"#;
        let page = format!("<div class='widget_document_library'>{page}</div><div class='qanda'><a href='/neighbour.pdf'>Conditions of approval</a></div>");
        let links = document_links(&page, &Url::parse("https://example.com/project/").unwrap());
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].url.as_str(), "https://example.com/letter");
        assert_eq!(
            links[1].url.as_str(),
            "https://example.com/project/other.pdf?a=1&b=2"
        );
    }

    #[test]
    fn migration_preserves_existing_projects_and_is_idempotent() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE Projects(Id TEXT PRIMARY KEY, Serialized TEXT); INSERT INTO Projects VALUES ('old', '{}');").unwrap();
        initialize_schema(&db).unwrap();
        initialize_schema(&db).unwrap();
        assert_eq!(count(&db, "Projects"), 1);
        assert_eq!(count(&db, "ApprovalEvents"), 0);
    }

    #[test]
    fn history_preserves_decisions_and_first_seen_without_fabricating_dates() {
        let db = Database::new_in_memory().unwrap();
        let mut p = project(CONDITIONAL);
        record_decisions(&db, &p, 100).unwrap();
        record_decisions(&db, &p, 200).unwrap();
        assert_eq!(count(&db, "ApprovalEvents"), 1);
        let row: (i64, i64, bool, String, String) = db.query_row(
            "SELECT FirstSeen, LastSeen, IsBaseline, DecisionDate, ApplicationNumber FROM ApprovalEvents", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).unwrap();
        assert_eq!(
            row,
            (100, 200, true, "2026-05-07".into(), "DP-2026-00114".into())
        );
        p.attributes.archival_reason_message = Some(ISSUED.into());
        record_decisions(&db, &p, 300).unwrap();
        assert_eq!(count(&db, "ApprovalEvents"), 2);
        let baseline: bool = db
            .query_row(
                "SELECT IsBaseline FROM ApprovalEvents WHERE Kind = 'permit_issued'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!baseline);
        p.attributes.archival_reason_message = None;
        record_decisions(&db, &p, 400).unwrap();
        assert_eq!(count(&db, "ApprovalEvents"), 2);
    }

    #[test]
    fn approval_after_initial_pending_scan_is_not_a_baseline() {
        let db = Database::new_in_memory().unwrap();
        record_decisions(&db, &project("Consultation has concluded"), 100).unwrap();
        record_decisions(&db, &project(CONDITIONAL), 200).unwrap();
        let baseline: bool = db
            .query_row("SELECT IsBaseline FROM ApprovalEvents", [], |r| r.get(0))
            .unwrap();
        assert!(!baseline);
    }

    #[test]
    fn pdf_versions_are_deduplicated_and_old_content_is_preserved() {
        let mut db = Database::new_in_memory().unwrap();
        let url = Url::parse("https://example.com/letter").unwrap();
        let link = DocumentLink {
            title: "Prior-to letter".into(),
            url: url.clone(),
        };
        let id = record_document(&db, "p", &link, 100).unwrap();
        record_pdf(&mut db, id, &url, b"%PDF-original", 100).unwrap();
        record_pdf(&mut db, id, &url, b"%PDF-original", 200).unwrap();
        record_pdf(&mut db, id, &url, b"%PDF-revised", 300).unwrap();
        record_pdf(&mut db, id, &url, b"%PDF-original", 400).unwrap();
        assert_eq!(count(&db, "ApprovalDocumentVersions"), 2);
        let row: (i64, i64, Vec<u8>) = db.query_row(
            "SELECT FirstSeen, LastSeen, Content FROM ApprovalDocumentVersions ORDER BY Id LIMIT 1", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(row, (100, 400, b"%PDF-original".to_vec()));
    }

    // Each server has a finite script and asserts every request. Tests use only localhost.
    async fn server(responses: Vec<(&'static str, u16, &'static str)>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for (path, status, body) in responses {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut buffer = [0; 8192];
                let n = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                assert!(
                    request.starts_with(&format!("GET {path} HTTP/1.1")),
                    "unexpected request: {request}"
                );
                let response = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (base, task)
    }

    fn client() -> Client {
        Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn downloads_wrapper_and_direct_pdf() {
        let (base, task) = server(vec![
            (
                "/letter",
                200,
                include_str!("../test_files/approval-download.html"),
            ),
            ("/letter/download", 200, "%PDF-1.7\nexample"),
            ("/direct.pdf", 200, "%PDF-1.4\nother"),
        ])
        .await;
        let (url, bytes) = download_pdf(&client(), &Url::parse(&format!("{base}/letter")).unwrap())
            .await
            .unwrap();
        assert_eq!(url.path(), "/letter/download");
        assert_eq!(bytes, b"%PDF-1.7\nexample");
        assert!(download_pdf(
            &client(),
            &Url::parse(&format!("{base}/direct.pdf")).unwrap()
        )
        .await
        .unwrap()
        .1
        .starts_with(b"%PDF-"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_error_pages_and_non_pdf_downloads() {
        let (base, task) = server(vec![
            ("/not-found", 404, "missing"),
            ("/login", 200, "<html>Please log in</html>"),
            (
                "/letter",
                200,
                include_str!("../test_files/approval-download.html"),
            ),
            (
                "/letter/download",
                200,
                "<html>Temporarily unavailable</html>",
            ),
        ])
        .await;
        for path in ["/not-found", "/login", "/letter"] {
            assert!(
                download_pdf(&client(), &Url::parse(&format!("{base}{path}")).unwrap())
                    .await
                    .is_err()
            );
        }
        task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_responses() {
        let (base, task) = server(vec![("/large", 200, "123456789")]).await;
        assert!(
            get_bytes(&client(), &Url::parse(&format!("{base}/large")).unwrap(), 8)
                .await
                .is_err()
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn retries_failed_download_and_keeps_the_decision_and_link() {
        let page =
            "<div class='widget_document_library'><a href='/letter'>Prior-to letter</a></div>";
        let (base, task) = server(vec![
            ("/project", 200, page),
            ("/letter", 503, "try later"),
            ("/project", 200, page),
            ("/letter", 200, "%PDF-recovered"),
        ])
        .await;
        let mut db = Database::new_in_memory().unwrap();
        let mut p = project(CONDITIONAL);
        p.links.self_link = format!("{base}/project");
        assert!(track_with_client(&mut db, &[p.clone()], &client(), 100)
            .await
            .is_err());
        assert_eq!(count(&db, "ApprovalEvents"), 1);
        assert_eq!(count(&db, "ApprovalDocuments"), 1);
        assert_eq!(count(&db, "ApprovalDocumentVersions"), 0);
        let error: Option<String> = db
            .query_row("SELECT LastError FROM ApprovalDocuments", [], |r| r.get(0))
            .unwrap();
        assert!(error.unwrap().contains("503"));
        track_with_client(&mut db, &[p], &client(), 101)
            .await
            .unwrap();
        assert_eq!(count(&db, "ApprovalDocumentVersions"), 1);
        let error: Option<String> = db
            .query_row("SELECT LastError FROM ApprovalDocuments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(error, None);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn discovers_late_documents_and_revisions_without_notice_changes() {
        let page =
            "<div class='widget_document_library'><a href='/letter'>Prior-to letter</a></div>";
        let (base, task) = server(vec![
            ("/project", 200, "No documents yet"),
            ("/project", 200, page),
            ("/letter", 200, "%PDF-first"),
            ("/project", 200, page),
            ("/letter", 200, "%PDF-revised"),
        ])
        .await;
        let mut db = Database::new_in_memory().unwrap();
        let mut p = project(CONDITIONAL);
        p.links.self_link = format!("{base}/project");
        track_with_client(&mut db, &[p.clone()], &client(), 100)
            .await
            .unwrap();
        // No request until the daily recheck; server would reject an unexpected request.
        track_with_client(&mut db, &[p.clone()], &client(), 101)
            .await
            .unwrap();
        assert_eq!(count(&db, "ApprovalDocuments"), 0);
        track_with_client(&mut db, &[p.clone()], &client(), 100 + RECHECK_SECONDS)
            .await
            .unwrap();
        assert_eq!(count(&db, "ApprovalDocumentVersions"), 1);
        track_with_client(&mut db, &[p], &client(), 100 + 2 * RECHECK_SECONDS)
            .await
            .unwrap();
        assert_eq!(count(&db, "ApprovalDocumentVersions"), 2);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn changed_notice_triggers_recheck_before_daily_interval() {
        let (base, task) = server(vec![("/project", 200, ""), ("/project", 200, "")]).await;
        let mut db = Database::new_in_memory().unwrap();
        let mut p = project(CONDITIONAL);
        p.links.self_link = format!("{base}/project");
        track_with_client(&mut db, &[p.clone()], &client(), 100)
            .await
            .unwrap();
        p.attributes.archival_reason_message = Some(ISSUED.into());
        track_with_client(&mut db, &[p], &client(), 101)
            .await
            .unwrap();
        assert_eq!(count(&db, "ApprovalEvents"), 2);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn project_failure_does_not_prevent_other_projects_being_collected() {
        let (base, task) = server(vec![("/bad", 500, "oops"), ("/good", 200, "")]).await;
        let mut db = Database::new_in_memory().unwrap();
        let mut bad = project(CONDITIONAL);
        bad.links.self_link = format!("{base}/bad");
        let mut good = bad.clone();
        good.id = "other".into();
        good.links.self_link = format!("{base}/good");
        assert!(track_with_client(&mut db, &[bad, good], &client(), 100)
            .await
            .is_err());
        assert_eq!(count(&db, "ApprovalEvents"), 2);
        let success: Option<i64> = db
            .query_row(
                "SELECT LastSuccess FROM ApprovalScans WHERE ProjectId = 'other'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(success, Some(100));
        task.await.unwrap();
    }
}
