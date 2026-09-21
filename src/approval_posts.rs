//! Durable, bounded conditions outbox. Baselines never become posts.
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::time::Duration;

use crate::{conditions, db::Database, llm};

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS ApprovalPostingConfig (Id INTEGER PRIMARY KEY CHECK(Id=1), Mode TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS ApprovalPostingBaselines (ProjectId TEXT PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS ApprovalPosts (
            Id INTEGER PRIMARY KEY, ProjectId TEXT NOT NULL, VersionId INTEGER NOT NULL,
            ContentHash TEXT NOT NULL, State TEXT NOT NULL, PostText TEXT,
            CreatedAt INTEGER NOT NULL, LastAttempt INTEGER, LastError TEXT,
            UNIQUE(ProjectId, ContentHash));
        CREATE TABLE IF NOT EXISTS ApprovalPostDeliveries (
            PostId INTEGER NOT NULL REFERENCES ApprovalPosts(Id), Channel TEXT NOT NULL,
            Status TEXT NOT NULL DEFAULT 'pending', Attempts INTEGER NOT NULL DEFAULT 0,
            LastAttempt INTEGER, LastError TEXT, PRIMARY KEY(PostId, Channel));")?;
    Ok(())
}

/// Changing destinations or re-enabling starts a fresh baseline, never a catch-up burst.
pub fn configure(db: &Connection, enabled: bool, slack: bool, bluesky: bool) -> Result<()> {
    let mode = if enabled {
        [slack.then_some("slack"), bluesky.then_some("bluesky")]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(",")
    } else {
        String::new()
    };
    let tx = db.unchecked_transaction()?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT Mode FROM ApprovalPostingConfig WHERE Id=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if previous.as_deref() != Some(&mode) {
        tx.execute("DELETE FROM ApprovalPostingBaselines", [])?;
        tx.execute("UPDATE ApprovalPosts SET State='suppressed' WHERE State IN ('pending','ready','generating')", [])?;
        tx.execute(
            "UPDATE ApprovalPostDeliveries SET Status='suppressed' WHERE Status='pending'",
            [],
        )?;
        tx.execute("INSERT INTO ApprovalPostingConfig VALUES (1,?1) ON CONFLICT(Id) DO UPDATE SET Mode=excluded.Mode", [mode])?;
    }
    tx.commit()?;
    Ok(())
}

/// Call only after a project's successful scan, including scans with no decision yet.
pub fn observe_project(db: &Connection, project: &str, now: i64) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    let mode: String = tx
        .query_row(
            "SELECT Mode FROM ApprovalPostingConfig WHERE Id=1",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let ready: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM ApprovalPostingBaselines WHERE ProjectId=?1)",
        [project],
        |r| r.get(0),
    )?;
    let versions = tx
        .prepare(
            "SELECT coalesce(max(CASE WHEN d.CurrentVersionId=v.Id THEN v.Id END), min(v.Id)), v.ContentHash, max(coalesce(d.CurrentVersionId=v.Id,0))
        FROM ApprovalDocumentVersions v JOIN ApprovalDocuments d ON d.Id=v.DocumentId
        WHERE d.ProjectId=?1 GROUP BY v.ContentHash ORDER BY min(v.Id)",
        )?
        .query_map([project], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (version, hash, current) in versions {
        let state = if ready && !mode.is_empty() && current {
            "pending"
        } else {
            "baseline"
        };
        let inserted = tx.execute("INSERT OR IGNORE INTO ApprovalPosts(ProjectId,VersionId,ContentHash,State,CreatedAt) VALUES(?1,?2,?3,?4,?5)", params![project, version, hash, state, now])?;
        if inserted > 0 && state == "pending" {
            let id = tx.last_insert_rowid();
            for channel in mode.split(',') {
                tx.execute(
                    "INSERT INTO ApprovalPostDeliveries(PostId,Channel) VALUES(?1,?2)",
                    params![id, channel],
                )?;
            }
        }
    }
    // Coalesce unsent revisions of one document. Keep partially delivered posts immutable.
    tx.execute("UPDATE ApprovalPosts SET State='superseded' WHERE ProjectId=?1 AND State IN ('pending','ready','generating')
        AND NOT EXISTS(SELECT 1 FROM ApprovalPostDeliveries x WHERE x.PostId=ApprovalPosts.Id AND x.Status IN ('sent','sending','uncertain'))
        AND NOT EXISTS(SELECT 1 FROM ApprovalDocumentVersions source
            JOIN ApprovalDocuments d ON d.Id=source.DocumentId
            JOIN ApprovalDocumentVersions current ON current.Id=d.CurrentVersionId
            WHERE source.Id=ApprovalPosts.VersionId AND current.ContentHash=ApprovalPosts.ContentHash)", [project])?;
    tx.execute("UPDATE ApprovalPostDeliveries SET Status='superseded' WHERE Status='pending' AND PostId IN (SELECT Id FROM ApprovalPosts WHERE State='superseded')", [])?;
    tx.execute(
        "INSERT OR IGNORE INTO ApprovalPostingBaselines VALUES(?1)",
        [project],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn has_baseline(db: &Connection, project: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM ApprovalPostingBaselines WHERE ProjectId=?1)",
        [project],
        |r| r.get(0),
    )?)
}

pub async fn prepare(db: &Database, limit: u32, now: i64) -> Result<Vec<String>> {
    let started = std::time::Instant::now();
    // A crashed worker can resume; a live worker has a five-minute overall timeout.
    db.execute(
        "UPDATE ApprovalPosts SET State='pending' WHERE State='generating' AND LastAttempt < ?1",
        [now - 900],
    )?;
    let ids = db
        .prepare(
            "SELECT Id, VersionId FROM ApprovalPosts WHERE State='pending' ORDER BY Id LIMIT ?1",
        )?
        .query_map([limit], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut failures = Vec::new();
    for (id, version) in ids {
        let claimed_at = now.saturating_add(started.elapsed().as_secs() as i64);
        if db.execute("UPDATE ApprovalPosts SET State='generating',LastAttempt=?2 WHERE Id=?1 AND State='pending'", params![id, claimed_at])? == 0 { continue; }
        let result = tokio::time::timeout(
            Duration::from_secs(300),
            conditions::summarize_conditions_with_model(db, 1, Some(version), false, llm::MODEL),
        )
        .await;
        let result = match result {
            Ok(result) => result.and_then(|mut run| {
                ensure!(run.failures.is_empty(), "{}", run.failures.join("; "));
                run.summaries
                    .pop()
                    .context("Summary retry budget exhausted")?
                    .post_text()
            }),
            Err(_) => Err(anyhow::anyhow!("Conditions summary exceeded five minutes")),
        };
        match result {
            Ok(text) => {
                set_text(db, id, &text)?;
            }
            Err(error) => {
                let error = format!("{error:#}");
                let attempts: i64 = db.query_row("SELECT coalesce(max(Attempts),0) FROM ConditionsSummaries WHERE DocumentVersionId=?1 AND Model=?2 AND PromptVersion=?3 AND ExtractorVersion=?4", params![version,llm::MODEL,conditions::PROMPT_VERSION,conditions::EXTRACTOR_VERSION], |r| r.get(0))?;
                db.execute("UPDATE ApprovalPosts SET State=?2,LastError=?3 WHERE Id=?1 AND State='generating'", params![id,if attempts >= 3 { "failed" } else { "pending" },error])?;
                failures.push(format!("Conditions post {id}: {error}"));
            }
        }
    }
    Ok(failures)
}

fn set_text(db: &Connection, id: i64, text: &str) -> Result<()> {
    ensure!(
        !text.trim().is_empty() && text.chars().count() <= 300,
        "Invalid conditions post length"
    );
    db.execute("UPDATE ApprovalPosts SET PostText=?2,State='ready',LastError=NULL WHERE Id=?1 AND State='generating'", params![id,text])?;
    Ok(())
}

#[derive(Debug)]
pub struct Delivery {
    pub id: i64,
    pub text: String,
    pub record_key: String,
    pub created_at: i64,
}

/// Claim before sending. Do not destructively pop messages before external I/O.
pub fn claim(db: &Connection, channel: &str, now: i64) -> Result<Option<Delivery>> {
    ensure!(
        matches!(channel, "slack" | "bluesky"),
        "Unknown destination"
    );
    let tx = db.unchecked_transaction()?;
    if channel == "slack" {
        // Incoming webhooks have no idempotency key. A crash after send is ambiguous.
        tx.execute("UPDATE ApprovalPostDeliveries SET Status='uncertain',LastError='Worker stopped during delivery; inspect destination before retrying' WHERE Channel='slack' AND Status='sending' AND LastAttempt < ?1", [now-300])?;
    } else {
        tx.execute("UPDATE ApprovalPostDeliveries SET Status='pending' WHERE Channel='bluesky' AND Status='sending' AND LastAttempt < ?1 AND Attempts < 3", [now-300])?;
        tx.execute("UPDATE ApprovalPostDeliveries SET Status='failed',LastError='Worker stopped during final delivery attempt' WHERE Channel='bluesky' AND Status='sending' AND LastAttempt < ?1 AND Attempts >= 3", [now-300])?;
    }
    let row = tx.query_row("SELECT p.Id,p.PostText,p.CreatedAt FROM ApprovalPosts p JOIN ApprovalPostDeliveries d ON d.PostId=p.Id
        WHERE p.State='ready' AND d.Channel=?1 AND d.Status='pending' AND d.Attempts<3
        AND (d.LastAttempt IS NULL OR d.LastAttempt<=?2) ORDER BY p.Id LIMIT 1", params![channel,now-300], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?))).optional()?;
    let delivery = if let Some((id, text, created_at)) = row {
        tx.execute("UPDATE ApprovalPostDeliveries SET Status='sending',Attempts=Attempts+1,LastAttempt=?3 WHERE PostId=?1 AND Channel=?2", params![id,channel,now])?;
        Some(Delivery {
            id,
            text,
            record_key: record_key(id, created_at)?,
            created_at,
        })
    } else {
        None
    };
    tx.commit()?;
    Ok(delivery)
}

fn record_key(id: i64, created_at: i64) -> Result<String> {
    // Posts require a TID, not an arbitrary hash. These persisted fields give retries
    // the same key and distinct rows within a scan different microsecond offsets.
    let time =
        chrono::DateTime::from_timestamp(created_at, (id.rem_euclid(1_000_000) * 1000) as u32)
            .context("Invalid conditions timestamp")?;
    Ok(
        bsky_sdk::api::types::string::Tid::from_datetime(513u32.try_into().unwrap(), time)
            .to_string(),
    )
}

#[derive(Debug)]
pub enum Outcome {
    Sent,
    Retry(String),
    RetryAt { message: String, at: i64 },
    Failed(String),
    Uncertain(String),
}

pub fn finish(db: &Connection, id: i64, channel: &str, outcome: &Outcome) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    let (status, error) = match outcome {
        Outcome::Sent => ("sent", None),
        Outcome::Retry(e) => ("pending", Some(e.as_str())),
        Outcome::RetryAt { message, .. } => ("pending", Some(message.as_str())),
        Outcome::Failed(e) => ("failed", Some(e.as_str())),
        Outcome::Uncertain(e) => ("uncertain", Some(e.as_str())),
    };
    tx.execute("UPDATE ApprovalPostDeliveries SET Status=CASE WHEN ?3='pending' AND Attempts>=3 THEN 'failed' ELSE ?3 END,LastError=?4 WHERE PostId=?1 AND Channel=?2 AND Status='sending'", params![id,channel,status,error])?;
    if let Outcome::RetryAt { at, .. } = outcome {
        tx.execute("UPDATE ApprovalPostDeliveries SET LastAttempt=max(LastAttempt,?3) WHERE PostId=?1 AND Channel=?2 AND Status='pending'",params![id,channel,at.saturating_sub(300)])?;
    }
    tx.execute("UPDATE ApprovalPosts SET State='sent' WHERE Id=?1 AND State='ready' AND NOT EXISTS(SELECT 1 FROM ApprovalPostDeliveries WHERE PostId=?1 AND Status<>'sent')",[id])?;
    tx.commit()?;
    Ok(())
}

pub async fn send_slack(url: &str, text: &str) -> Outcome {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(client) => client,
        Err(error) => return Outcome::Retry(error.to_string()),
    };
    match client
        .post(url)
        .json(&serde_json::json!({"text":text,"unfurl_links":false,"unfurl_media":false}))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => Outcome::Sent,
        Ok(response) if response.status().as_u16() == 429 => {
            let delay = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(300)
                .max(300);
            Outcome::RetryAt {
                message: "Slack rate limited the conditions post".into(),
                at: chrono::Utc::now().timestamp().saturating_add(delay),
            }
        }
        Ok(response) if response.status().is_server_error() => Outcome::Uncertain(format!(
            "Slack returned {}; check whether it accepted the post",
            response.status()
        )),
        Ok(response) => Outcome::Failed(format!(
            "Slack rejected conditions post: {}",
            response.status()
        )),
        Err(error) if error.is_connect() => Outcome::Retry("Could not connect to Slack".into()),
        Err(_) => Outcome::Uncertain(
            "Slack delivery interrupted; check whether it accepted the post".into(),
        ),
    }
}

pub fn status_counts(db: &Connection) -> Result<serde_json::Value> {
    let mut result = serde_json::Map::new();
    let mut stmt = db.prepare(
        "SELECT Channel,Status,count(*) FROM ApprovalPostDeliveries GROUP BY Channel,Status",
    )?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })? {
        let (channel, status, count) = row?;
        result.insert(format!("{channel}_{status}"), count.into());
    }
    let mut stmt = db.prepare("SELECT State,count(*) FROM ApprovalPosts GROUP BY State")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (state, count) = row?;
        result.insert(format!("posts_{state}"), count.into());
    }
    Ok(result.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(db: &Connection, project: &str, document: i64, body: &str) -> i64 {
        db.execute("INSERT OR IGNORE INTO ApprovalDocuments(Id,ProjectId,SourceUrl,Title,FirstSeen,LastSeen) VALUES(?1,?2,?3,'Test application',1,1)",params![document,project,format!("https://example.com/{document}")]).unwrap();
        let hash = crate::approval_storage::content_hash(body.as_bytes());
        db.execute(
            "INSERT OR IGNORE INTO ApprovalPdfContent VALUES(?1,?2)",
            params![hash, body.as_bytes()],
        )
        .unwrap();
        db.execute("INSERT INTO ApprovalDocumentVersions(DocumentId,DownloadUrl,FirstSeen,LastSeen,Content,ContentHash) VALUES(?1,'https://example.com/pdf',1,1,X'',?2)",params![document,hash]).unwrap();
        let id = db.last_insert_rowid();
        db.execute(
            "UPDATE ApprovalDocuments SET CurrentVersionId=?2 WHERE Id=?1",
            params![document, id],
        )
        .unwrap();
        id
    }

    fn count(db: &Connection, state: &str) -> i64 {
        db.query_row(
            "SELECT count(*) FROM ApprovalPosts WHERE State=?1",
            [state],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn setup() -> Database {
        let db = Database::new_in_memory().unwrap();
        configure(&db, true, true, true).unwrap();
        db
    }

    fn pending(db: &Database) -> i64 {
        observe_project(db, "p", 10).unwrap();
        archive(db, "p", 1, "new conditions");
        observe_project(db, "p", 20).unwrap();
        db.query_row(
            "SELECT Id FROM ApprovalPosts WHERE State='pending'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn ready(db: &Database) -> i64 {
        let id = pending(db);
        db.execute(
            "UPDATE ApprovalPosts SET State='generating' WHERE Id=?1",
            [id],
        )
        .unwrap();
        set_text(
            db,
            id,
            "Approval conditions for 1 Test St: provide five bike spaces.\nhttps://example.com/pdf",
        )
        .unwrap();
        id
    }

    #[test]
    fn first_scan_and_first_seen_approved_projects_never_queue_history() {
        let db = setup();
        for n in 1..=1000 {
            archive(&db, &format!("p{n}"), n, "existing letter");
        }
        for n in 1..=1000 {
            observe_project(&db, &format!("p{n}"), 10).unwrap();
        }
        assert_eq!(count(&db, "baseline"), 1000);
        assert_eq!(count(&db, "pending"), 0);
        assert!(claim(&db, "slack", 20).unwrap().is_none());
        assert!(claim(&db, "bluesky", 20).unwrap().is_none());
        // A later cron run with the same configuration must not replay history.
        configure(&db, true, true, true).unwrap();
        for n in 1..=1000 {
            observe_project(&db, &format!("p{n}"), 25).unwrap();
        }
        assert_eq!(count(&db, "baseline"), 1000);
        assert_eq!(count(&db, "pending"), 0);
        archive(
            &db,
            "later-discovered-project",
            1001,
            "also already approved",
        );
        observe_project(&db, "later-discovered-project", 30).unwrap();
        assert_eq!(count(&db, "pending"), 0);
    }

    #[test]
    fn pending_application_then_approval_queues_once_per_project_and_content() {
        let db = setup();
        pending(&db);
        observe_project(&db, "p", 30).unwrap();
        archive(&db, "p", 2, "new conditions"); // Same bytes at another URL.
        observe_project(&db, "p", 40).unwrap();
        assert_eq!(count(&db, "pending"), 1);
        assert_eq!(
            db.query_row("SELECT count(*) FROM ApprovalPostDeliveries", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn reenable_and_destination_changes_rebaseline_without_a_backlog() {
        let db = setup();
        ready(&db);
        configure(&db, false, true, true).unwrap();
        assert!(claim(&db, "slack", 30).unwrap().is_none());
        archive(&db, "p", 1, "changed while disabled");
        configure(&db, true, true, true).unwrap();
        observe_project(&db, "p", 40).unwrap();
        assert_eq!(count(&db, "pending"), 0);
        archive(&db, "p", 1, "later update");
        observe_project(&db, "p", 50).unwrap();
        assert_eq!(count(&db, "pending"), 1);
        configure(&db, true, true, false).unwrap();
        observe_project(&db, "p", 60).unwrap();
        assert_eq!(count(&db, "pending"), 0);
    }

    #[test]
    fn unsent_revisions_are_superseded_but_partial_deliveries_are_immutable() {
        let db = setup();
        let first = ready(&db);
        archive(&db, "p", 1, "revision two");
        observe_project(&db, "p", 30).unwrap();
        assert_eq!(count(&db, "superseded"), 1);
        assert_eq!(count(&db, "pending"), 1);
        assert!(claim(&db, "slack", 40).unwrap().is_none());
        db.execute(
            "UPDATE ApprovalPosts SET State='generating' WHERE State='pending'",
            [],
        )
        .unwrap();
        let second = db
            .query_row(
                "SELECT Id FROM ApprovalPosts WHERE State='generating'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        set_text(&db, second, "Revised conditions.\nhttps://example.com/pdf").unwrap();
        let sent = claim(&db, "slack", 50).unwrap().unwrap();
        assert_ne!(first, sent.id);
        finish(&db, sent.id, "slack", &Outcome::Sent).unwrap();
        archive(&db, "p", 1, "revision three");
        observe_project(&db, "p", 60).unwrap();
        assert_eq!(claim(&db, "bluesky", 70).unwrap().unwrap().id, second);
    }

    #[test]
    fn delivery_claims_survive_restart_and_retries_do_not_repeat_other_channel() {
        let db = setup();
        ready(&db);
        let slack = claim(&db, "slack", 1000).unwrap().unwrap();
        finish(&db, slack.id, "slack", &Outcome::Sent).unwrap();
        let bsky = claim(&db, "bluesky", 1000).unwrap().unwrap();
        assert!(claim(&db, "bluesky", 1100).unwrap().is_none());
        // Simulate a crash after the remote write, before marking sent.
        let retried = claim(&db, "bluesky", 1400).unwrap().unwrap();
        assert_eq!(bsky.record_key, retried.record_key);
        assert_eq!(bsky.text, retried.text);
        assert_eq!(bsky.created_at, retried.created_at);
        finish(&db, retried.id, "bluesky", &Outcome::Sent).unwrap();
        assert!(claim(&db, "slack", 2000).unwrap().is_none());
        assert!(claim(&db, "bluesky", 2000).unwrap().is_none());
    }

    #[test]
    fn uncertain_slack_is_held_and_confirmed_failures_have_a_retry_budget() {
        let db = setup();
        let id = ready(&db);
        claim(&db, "slack", 1000).unwrap().unwrap();
        assert!(claim(&db, "slack", 1400).unwrap().is_none());
        assert_eq!(status_counts(&db).unwrap()["slack_uncertain"], 1);
        for time in [1000, 1400, 1800] {
            claim(&db, "bluesky", time).unwrap().unwrap();
            finish(&db, id, "bluesky", &Outcome::Retry("unavailable".into())).unwrap();
        }
        assert!(claim(&db, "bluesky", 2200).unwrap().is_none());
        assert_eq!(status_counts(&db).unwrap()["bluesky_failed"], 1);
    }

    #[test]
    fn rate_limit_retry_time_is_persisted() {
        let db = setup();
        let id = ready(&db);
        claim(&db, "slack", 1000).unwrap().unwrap();
        finish(
            &db,
            id,
            "slack",
            &Outcome::RetryAt {
                message: "rate limit".into(),
                at: 5000,
            },
        )
        .unwrap();
        assert!(claim(&db, "slack", 4999).unwrap().is_none());
        assert_eq!(claim(&db, "slack", 5000).unwrap().unwrap().id, id);
    }

    #[test]
    fn configuration_change_during_generation_cannot_release_a_post() {
        let db = setup();
        let id = pending(&db);
        db.execute(
            "UPDATE ApprovalPosts SET State='generating' WHERE Id=?1",
            [id],
        )
        .unwrap();
        configure(&db, false, true, true).unwrap();
        set_text(
            &db,
            id,
            "A completed but cancelled draft.\nhttps://example.com/pdf",
        )
        .unwrap();
        assert!(claim(&db, "slack", 1000).unwrap().is_none());
    }

    #[test]
    fn removed_letter_cancels_unsent_post_and_tids_are_valid_and_stable() {
        let db = setup();
        ready(&db);
        db.execute("UPDATE ApprovalDocuments SET CurrentVersionId=NULL", [])
            .unwrap();
        observe_project(&db, "p", 30).unwrap();
        assert!(claim(&db, "slack", 40).unwrap().is_none());
        assert_eq!(count(&db, "superseded"), 1);
        let first = record_key(1, 1_789_920_000).unwrap();
        assert!(first.parse::<bsky_sdk::api::types::string::Tid>().is_ok());
        assert_eq!(first, record_key(1, 1_789_920_000).unwrap());
        assert_ne!(first, record_key(2, 1_789_920_000).unwrap());
    }

    #[tokio::test]
    async fn cached_summaries_are_bounded_and_no_baseline_triggers_generation() {
        let db = setup();
        for n in 1..=1000 {
            let project = format!("p{n}");
            observe_project(&db, &project, 10).unwrap();
            let version = archive(&db, &project, n, "new letter");
            observe_project(&db, &project, 20).unwrap();
            db.execute("INSERT INTO ConditionsSummaries(DocumentVersionId,Model,PromptVersion,ExtractorVersion,SummaryJson) VALUES(?1,?2,?3,?4,?5)",params![version,llm::MODEL,conditions::PROMPT_VERSION,conditions::EXTRACTOR_VERSION,
                r#"{"overview":"Approval conditions for 1 Test St require five bike spaces.","requirements":[],"limitations":[]}"#]).unwrap();
        }
        assert!(prepare(&db, 3, 30).await.unwrap().is_empty());
        assert_eq!(count(&db, "ready"), 3);
        assert_eq!(count(&db, "pending"), 997);
        // Even an unexpectedly large eligible backlog cannot all reach delivery.
        for channel in ["slack", "bluesky"] {
            for _ in 0..3 {
                let delivery = claim(&db, channel, 31).unwrap().unwrap();
                finish(&db, delivery.id, channel, &Outcome::Sent).unwrap();
            }
            assert!(claim(&db, channel, 31).unwrap().is_none());
        }
        assert_eq!(count(&db, "sent"), 3);
        configure(&db, false, true, true).unwrap();
        assert!(prepare(&db, 3, 40).await.unwrap().is_empty());
        assert_eq!(count(&db, "ready"), 0);
    }

    #[tokio::test]
    async fn local_slack_transport_preserves_text_and_classifies_ambiguous_failures() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        for status in [200, 400, 429, 500] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = vec![0; 8192];
                let n = socket.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..n]);
                assert!(request.contains("Approval conditions"));
                assert!(request.contains("https://example.com/pdf"));
                socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").as_bytes()).await.unwrap();
            });
            let result = send_slack(&url, "Approval conditions.\nhttps://example.com/pdf").await;
            match status {
                200 => assert!(matches!(result, Outcome::Sent)),
                400 => assert!(matches!(result, Outcome::Failed(_))),
                429 => assert!(matches!(result, Outcome::RetryAt { .. })),
                500 => assert!(matches!(result, Outcome::Uncertain(_))),
                _ => unreachable!(),
            }
            server.await.unwrap();
        }
    }
}
