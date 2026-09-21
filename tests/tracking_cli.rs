use rezoning_scraper::{db::Database, models::Projects, queue::Queue};
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rezoning-tracking-test-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn tracking_only_never_consumes_existing_queues_or_updates_project_snapshots() {
    let scratch = Scratch::new();
    let database_path = scratch.0.join("test.db");
    let fixture_path = scratch.0.join("projects.json");
    let mut projects: Projects =
        serde_json::from_str(include_str!("../test_files/ExampleInput.json")).unwrap();
    let mut pending = projects
        .data
        .iter()
        .find(|p| p.attributes.name.contains("development application"))
        .unwrap()
        .clone();
    pending.attributes.archival_reason_message = Some("Consultation has concluded".into());
    pending.attributes.description = None;
    // Any unexpected attempt to use this URL would fail. Pending applications need no page fetch.
    pending.links.self_link = "http://127.0.0.1:1/must-not-fetch".into();
    projects.data = vec![pending.clone()];
    std::fs::write(&fixture_path, serde_json::to_vec(&projects).unwrap()).unwrap();

    let mut db = Database::new_from_file(database_path.to_str().unwrap()).unwrap();
    let mut original = pending.clone();
    original.attributes.name = "Existing project snapshot".into();
    db.upsert_projects(&[original]).unwrap();
    // Invalid payloads ensure accidentally entering any queue processor would also fail.
    for name in ["llm_queue", "slack_post_queue", "bluesky_post_queue"] {
        Queue::<String>::new(name, &db)
            .push(&db, "must not process".into())
            .unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_rezoning-scraper"))
        .env_clear()
        .env("SLACK_WEBHOOK_URL", "http://127.0.0.1:1/must-not-post")
        .env("BLUESKY_USER", "fake-test-user")
        .env("BLUESKY_PASSWORD", "fake-test-password")
        .env("OPEN_ROUTER_API_KEY", "fake-test-key")
        .env(
            "DOGSTATSD_ADDRESS",
            "invalid address: tracking-only must not initialize monitoring",
        )
        .args(["--tracking-only", "--database"])
        .arg(&database_path)
        .arg("--projects-file")
        .arg(&fixture_path)
        .current_dir(&scratch.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Approval tracking complete"));
    for name in ["llm_queue", "slack_post_queue", "bluesky_post_queue"] {
        assert_eq!(Queue::<String>::new(name, &db).depth(&db).unwrap(), 1);
    }
    assert_eq!(
        db.get_project(&pending.id).unwrap().attributes.name,
        "Existing project snapshot"
    );
    let scans: i64 = db
        .query_row("SELECT COUNT(*) FROM ApprovalScans", [], |r| r.get(0))
        .unwrap();
    assert_eq!(scans, 1);
    assert!(!scratch.0.join("rezoning_scraper.db").exists());
}

#[test]
fn rejects_conflicting_or_unsafe_cli_combinations_before_running() {
    for args in [
        vec!["--tracking-only", "--skip-update-db"],
        vec!["--track-approvals", "--skip-update-db"],
        vec!["--tracking-only", "--monitoring-test", "ok"],
        vec!["--projects-file", "anything.json"],
        vec!["--summarize-conditions", "--tracking-only"],
        vec!["--summarize-conditions", "--track-approvals"],
        vec!["--summarize-conditions", "--skip-update-db"],
        vec!["--summarize-conditions", "--monitoring-test", "ok"],
        vec!["--summarize-conditions", "--summary-limit", "0"],
        vec!["--summarize-conditions", "--summary-limit", "101"],
        vec!["--summarize-conditions", "--document-version", "-1"],
        vec!["--document-version", "1"],
        vec!["--summary-limit", "2"],
        vec!["--conditions-model", "another-model"],
        vec![
            "--summarize-conditions",
            "--conditions-model",
            "gpt-6-astra",
        ],
        vec![
            "--summarize-conditions",
            "--conditions-model",
            "open_router::openai/gpt-5",
        ],
        vec![
            "--summarize-conditions",
            "--conditions-model",
            "open_router::openrouter/auto",
        ],
        vec!["--retry-failed-summaries"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rezoning-scraper"))
            .env_clear()
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "unexpected success: {args:?}"
        );
    }
}

fn seed_archived_document(db: &Database) {
    db.execute(
        "INSERT INTO ApprovalDocuments(Id, ProjectId, SourceUrl, Title, FirstSeen, LastSeen)
        VALUES (1, 'test', 'https://example.com/conditions.pdf', 'Test conditions', 1, 1)",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO ApprovalDocumentVersions(Id, DocumentId, DownloadUrl, FirstSeen, LastSeen, Content)
        VALUES (1, 1, 'https://example.com/conditions.pdf', 1, 1, ?1)",
        [include_bytes!("../test_files/conditions-renfrew.pdf").as_slice()]).unwrap();
}

#[test]
fn displaying_cached_conditions_summary_never_processes_posting_queues() {
    let scratch = Scratch::new();
    let path = scratch.0.join("test.db");
    let db = Database::new_from_file(path.to_str().unwrap()).unwrap();
    seed_archived_document(&db);
    for name in ["llm_queue", "slack_post_queue", "bluesky_post_queue"] {
        Queue::<String>::new(name, &db)
            .push(&db, "must not process".into())
            .unwrap();
    }
    let summary = serde_json::json!({"overview":"Test childcare approval requires five Class B bicycle spaces.", "requirements":[{
        "requirement":"Provide five Class B bicycle spaces.", "page":2,
        "evidence":"Provide the required five (5) Class B bicycle spaces"}], "limitations":[]});
    db.execute("INSERT INTO ConditionsSummaries(DocumentVersionId, Model, PromptVersion, ExtractorVersion, SummaryJson, Attempts, CompletedAt)
        VALUES (1, 'open_router::z-ai/glm-5.3-flash', ?2, 'pdf-extract-0.12.1-v2', ?1, 1, 1)", rusqlite::params![summary.to_string(), rezoning_scraper::conditions::PROMPT_VERSION]).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rezoning-scraper"))
        .env_clear()
        .env("SLACK_WEBHOOK_URL", "http://127.0.0.1:1/must-not-post")
        .env("BLUESKY_USER", "fake-test-user")
        .env("BLUESKY_PASSWORD", "fake-test-password")
        .env(
            "DOGSTATSD_ADDRESS",
            "invalid address: summaries must not initialize monitoring",
        )
        .args([
            "--summarize-conditions",
            "--document-version",
            "1",
            "--database",
        ])
        .arg(&path)
        .current_dir(&scratch.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim_end(), "Test childcare approval requires five Class B bicycle spaces.\nhttps://example.com/conditions.pdf");
    assert!(stdout.trim_end().chars().count() <= 300);
    for name in ["llm_queue", "slack_post_queue", "bluesky_post_queue"] {
        assert_eq!(Queue::<String>::new(name, &db).depth(&db).unwrap(), 1);
    }
    let attempts: i64 = db
        .query_row("SELECT Attempts FROM ConditionsSummaries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(attempts, 1);
}

#[test]
fn missing_key_does_not_consume_summary_attempts() {
    let scratch = Scratch::new();
    let path = scratch.0.join("test.db");
    let db = Database::new_from_file(path.to_str().unwrap()).unwrap();
    seed_archived_document(&db);
    let output = Command::new(env!("CARGO_BIN_EXE_rezoning-scraper"))
        .env_clear()
        .env("OPENAI_API_KEY", "must-not-use-another-provider-key")
        .args(["--summarize-conditions", "--database"])
        .arg(&path)
        .current_dir(&scratch.0)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("OPEN_ROUTER_API_KEY is required"));
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM ConditionsSummaries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn storage_report_migrates_legacy_bytes_without_processing_any_queues() {
    let scratch = Scratch::new();
    let path = scratch.0.join("archive.db");
    let db = Database::new_from_file(path.to_str().unwrap()).unwrap();
    seed_archived_document(&db);
    let before: Vec<u8> = db
        .query_row(
            "SELECT Content FROM ApprovalDocumentVersions WHERE Id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    for name in ["llm_queue", "slack_post_queue", "bluesky_post_queue"] {
        Queue::<String>::new(name, &db)
            .push(&db, "must not process".into())
            .unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_rezoning-scraper"))
        .env_clear()
        .env("POST_CONDITIONS", "true")
        .env("SLACK_WEBHOOK_URL", "http://127.0.0.1:1/must-not-post")
        .env("BLUESKY_USER", "fake-test-user")
        .env("BLUESKY_PASSWORD", "fake-test-password")
        .env(
            "DOGSTATSD_ADDRESS",
            "invalid:must-not-initialize-monitoring",
        )
        .args(["--approval-storage-stats", "--database"])
        .arg(&path)
        .current_dir(&scratch.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["storage"]["unique_pdfs"], 1);
    assert_eq!(report["storage"]["pdf_bytes"], before.len());
    assert_eq!(
        rezoning_scraper::approval_storage::read_pdf(&db, 1).unwrap(),
        before
    );
    for name in ["llm_queue", "slack_post_queue", "bluesky_post_queue"] {
        assert_eq!(Queue::<String>::new(name, &db).depth(&db).unwrap(), 1);
    }
    assert_eq!(
        db.query_row("SELECT count(*) FROM ApprovalPosts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn condition_posting_environment_and_limits_are_validated_before_running() {
    let scratch = Scratch::new();
    for arguments in [
        vec!["--post-conditions=invalid"],
        vec!["--conditions-post-limit=0"],
        vec!["--conditions-post-limit=21"],
        vec!["--approval-storage-stats", "--summarize-conditions"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rezoning-scraper"))
            .env_clear()
            .args(arguments)
            .current_dir(&scratch.0)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!scratch.0.join("rezoning_scraper.db").exists());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_rezoning-scraper"))
        .env_clear()
        .env("POST_CONDITIONS", "invalid")
        .arg("--approval-storage-stats")
        .current_dir(&scratch.0)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!scratch.0.join("rezoning_scraper.db").exists());
    for value in ["true", "false"] {
        let output = Command::new(env!("CARGO_BIN_EXE_rezoning-scraper"))
            .env_clear()
            .env("POST_CONDITIONS", value)
            .arg("--approval-storage-stats")
            .current_dir(&scratch.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
