use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, ValueEnum};
use colored::Colorize;
use db::Database;
use indicatif::ProgressBar;
use models::{Project, Projects, SummarizedProject};
use monitoring::{Monitoring, ServiceCheckStatus};
use queue::Queue;
use serde_json::json;
use std::time::Duration;
use summarizer::project_to_tweet;
use tokio::time::sleep;

mod approval_posts;
mod approval_storage;
mod approvals;
mod bluesky;
mod conditions;
mod db;
mod hive;
mod llm;
mod models;
mod monitoring;
mod queue;
mod summarizer;

const MAX_MESSAGE_PROCESSING_ATTEMPTS: i32 = 3;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum MonitoringTestStatus {
    Ok,
    Critical,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(
        long,
        help = "A Slack Incoming Webhook URL. If specified, will post info about new+modified rezonings to this address.",
        env = "SLACK_WEBHOOK_URL"
    )]
    slack_webhook_url: Option<String>,

    #[arg(
        long,
        help = "Bluesky username. Required for posting to Bluesky",
        env = "BLUESKY_USER"
    )]
    bluesky_user: Option<String>,
    #[arg(
        long,
        help = "Bluesky password. Required for posting to Bluesky",
        env = "BLUESKY_PASSWORD"
    )]
    bluesky_password: Option<String>,

    #[arg(long, help = "Skip updating the local database (useful for testing)")]
    skip_update_db: bool,

    #[arg(
        long,
        help = "Collect approval history even when automatic conditions posting is disabled",
        conflicts_with = "skip_update_db"
    )]
    track_approvals: bool,

    #[arg(long, env = "POST_CONDITIONS", default_value_t = true, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", help = "Automatically collect and post new conditions; set false to disable")]
    post_conditions: bool,

    #[arg(long, env = "CONDITIONS_POST_LIMIT", default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=20), help = "Maximum conditions summaries and deliveries per destination per run")]
    conditions_post_limit: u32,

    #[arg(long, conflicts_with_all = ["tracking_only", "track_approvals", "summarize_conditions", "monitoring_test", "projects_file"], help = "Print PDF storage and conditions delivery counts without crawling, generating or posting")]
    approval_storage_stats: bool,

    #[arg(long, help = "Only collect approvals; bypass all summarizing and posting queues", conflicts_with_all = ["skip_update_db", "monitoring_test"])]
    tracking_only: bool,

    #[arg(long, help = "Summarize archived conditions PDFs locally; do not crawl or post", conflicts_with_all = ["tracking_only", "track_approvals", "projects_file", "skip_update_db", "monitoring_test"])]
    summarize_conditions: bool,

    #[arg(long, default_value_t = 10, requires = "summarize_conditions", value_parser = clap::value_parser!(u32).range(1..=100), help = "Maximum PDFs to summarize in one run")]
    summary_limit: u32,

    #[arg(
        long,
        requires = "summarize_conditions",
        default_value = conditions::analysis::DEFAULT_MODEL,
        value_parser = llm::parse_model,
        help = "Model used for every conditions-analysis stage"
    )]
    conditions_model: String,

    #[arg(long, requires = "summarize_conditions", value_parser = clap::value_parser!(i64).range(1..), help = "Summarize one archived PDF version, or show its cached summary")]
    document_version: Option<i64>,

    #[arg(
        long,
        requires = "summarize_conditions",
        help = "Retry summaries that have already failed three times"
    )]
    retry_failed_summaries: bool,

    #[arg(
        long,
        default_value = "rezoning_scraper.db",
        help = "SQLite database path"
    )]
    database: String,

    #[arg(
        long,
        requires = "tracking_only",
        help = "Read a saved EngagementHQ-format projects response instead of crawling the site (document downloads still use HTTP)"
    )]
    projects_file: Option<std::path::PathBuf>,

    #[arg(
        long,
        value_enum,
        help = "Send a test status to monitoring without running the scraper"
    )]
    monitoring_test: Option<MonitoringTestStatus>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    if args.approval_storage_stats {
        let db = Database::new_from_file(&args.database)?;
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"storage":approval_storage::stats(&db)?,"deliveries":approval_posts::status_counts(&db)?})
            )?
        );
        return Ok(());
    }
    if args.summarize_conditions {
        return runtime.block_on(conditions::run_command(
            &args.database,
            args.summary_limit as usize,
            args.document_version,
            args.retry_failed_summaries,
            &args.conditions_model,
        ));
    }
    if args.tracking_only {
        return runtime.block_on(async_main(args, None));
    }
    let monitoring = Monitoring::new()?;

    if let Some(status) = args.monitoring_test {
        return test_monitoring(&monitoring, status);
    }

    let result = runtime.block_on(async_main(args, Some(&monitoring)));

    match result {
        Ok(()) => monitoring.service_check(
            ServiceCheckStatus::Ok,
            "Rezoning scraper completed successfully",
        ),
        Err(error) => {
            monitoring.error("Rezoning scraper run failed", &error, &[]);
            if let Err(reporting_error) = monitoring.service_check(
                ServiceCheckStatus::Critical,
                &format!("Rezoning scraper failed: {error:#}"),
            ) {
                monitoring.error(
                    "Failed to report scraper failure to DogStatsD",
                    &reporting_error,
                    &[],
                );
            }
            Err(error)
        }
    }
}

fn test_monitoring(monitoring: &Monitoring, status: MonitoringTestStatus) -> Result<()> {
    match status {
        MonitoringTestStatus::Ok => monitoring.service_check(
            ServiceCheckStatus::Ok,
            "Manual monitoring test: everything is OK",
        ),
        MonitoringTestStatus::Critical => {
            let error = anyhow!("deliberate monitoring test failure");
            monitoring.error(
                "Manual monitoring test",
                &error,
                &[("monitoring_test", json!(true))],
            );
            monitoring.service_check(
                ServiceCheckStatus::Critical,
                "Manual monitoring test: deliberate failure",
            )
        }
    }
}

async fn async_main(args: Args, monitoring: Option<&Monitoring>) -> Result<()> {
    println!(
        "{}",
        format!("Rezoning Scraper v{}", env!("CARGO_PKG_VERSION"))
            .bold()
            .green()
    );

    if !args.tracking_only && args.slack_webhook_url.is_none() {
        eprintln!(
            "{}",
            "Slack URI not specified; will not publish updates to Slack.".yellow()
        );
    }

    if !args.tracking_only && (args.bluesky_user.is_none() || args.bluesky_password.is_none()) {
        eprintln!("Bluesky username and password are required; will not post to Bluesky.");
    }

    let mut db = Database::new_from_file(&args.database)?;

    // Persist the kill switch even if fetching or ordinary processing later fails.
    if !args.tracking_only && !args.skip_update_db {
        approval_posts::configure(
            &db,
            args.post_conditions,
            args.slack_webhook_url.is_some(),
            args.bluesky_user.is_some() && args.bluesky_password.is_some(),
        )?;
    }

    // Fetch projects
    let start = std::time::Instant::now();
    let mut source_failures = Vec::new();
    let latest_projects = if let Some(path) = &args.projects_file {
        println!("Reading saved projects from {}...", path.display());
        let response: Projects = serde_json::from_slice(&std::fs::read(path)?)?;
        response.data
    } else {
        println!("{}", "Querying Shape Your City...".bold().cyan());
        let spinner = ProgressBar::new_spinner();
        spinner.enable_steady_tick(Duration::from_millis(100));
        spinner.set_message("Fetching project listings...");
        let stored = db.get_projects_by_id()?;
        let fetched = hive::fetch_projects(&hive::client()?, hive::BASE_URL, &stored).await?;
        spinner.finish_with_message(format!(
            "Fetched listings and {} project page(s)",
            fetched.detail_fetches
        ));
        source_failures = fetched.failures;
        fetched.projects
    };
    println!(
        "Retrieved {} projects in {}ms",
        format!("{}", latest_projects.len()).green(),
        format!("{}", start.elapsed().as_millis()).green()
    );

    if args.tracking_only {
        approvals::track_approvals(&mut db, &latest_projects).await?;
        println!("Approval tracking complete; no approval updates were queued for posting.");
    }
    if args.tracking_only {
        return Ok(());
    }
    let monitoring = monitoring.context("Monitoring is required for a normal scraper run")?;

    // Check if this is first run
    let is_initialization = db.is_empty()?;
    if is_initialization {
        println!(
            "{}",
            "First run detected - initializing database..."
                .bold()
                .yellow()
        );
    }

    // Compare against database
    println!("{}", "Comparing against local database...".bold().cyan());
    let compare_spinner = ProgressBar::new_spinner();
    compare_spinner.set_message("Comparing projects...");
    compare_spinner.enable_steady_tick(Duration::from_millis(100));
    let start = std::time::Instant::now();

    let mut new_projects = Vec::new();
    let mut changed_projects = Vec::new();

    for project in &latest_projects {
        if db.contains_project(&project.id)? {
            let old_version = db.get_project(&project.id)?;

            let mut changes = Vec::new();

            // Compare important fields
            if old_version.attributes.name != project.attributes.name {
                changes.push(ProjectChange {
                    field: "name".to_string(),
                    old_value: old_version.attributes.name,
                    new_value: project.attributes.name.clone(),
                });
            }
            if old_version.attributes.permalink != project.attributes.permalink {
                changes.push(ProjectChange {
                    field: "permalink".to_string(),
                    old_value: old_version.attributes.permalink,
                    new_value: project.attributes.permalink.clone(),
                });
            }
            if old_version.attributes.state != project.attributes.state {
                changes.push(ProjectChange {
                    field: "state".to_string(),
                    old_value: old_version.attributes.state,
                    new_value: project.attributes.state.clone(),
                });
            }
            if old_version.attributes.description != project.attributes.description {
                changes.push(ProjectChange {
                    field: "description".to_string(),
                    old_value: old_version.attributes.description.unwrap_or_default(),
                    new_value: project.attributes.description.clone().unwrap_or_default(),
                });
            }

            if !changes.is_empty() {
                changed_projects.push((project.clone(), changes));
            }
        } else {
            new_projects.push(project.clone());
        }
    }

    compare_spinner.finish_with_message(format!(
        "Compared {} projects to existing ones in {}ms",
        latest_projects.len(),
        start.elapsed().as_millis()
    ));

    if !args.skip_update_db {
        // Update database in a single transaction
        let start = std::time::Instant::now();
        db.upsert_projects(&latest_projects)?;
        let linked = db.link_legacy_projects()?;
        if linked > 0 {
            println!("Linked {linked} project(s) to their EngagementHQ-era records");
        }
        println!(
            "Updated database with {} projects in {}ms",
            format!("{}", latest_projects.len()).green(),
            format!("{}", start.elapsed().as_millis()).green()
        );
    }

    println!(
        "Found {} new projects and {} modified projects",
        new_projects.len().to_string().green(),
        changed_projects.len().to_string().yellow()
    );

    print_projects(&new_projects, &changed_projects);

    let llm_queue: Queue<Project> = Queue::new("llm_queue", &db);
    let slack_queue: Queue<SummarizedProject> = Queue::new("slack_post_queue", &db);
    let bsky_queue: Queue<SummarizedProject> = Queue::new("bluesky_post_queue", &db);

    if !is_initialization {
        for project in &new_projects {
            llm_queue.push(&db, project.clone())?;
        }
    }

    let mut processing_failures = 0;
    for failure in &source_failures {
        monitoring.error(
            "Failed to fetch project page",
            &anyhow!("{failure}"),
            &[("source", json!("shapeyourcity"))],
        );
        processing_failures += 1;
    }

    // Process LLM queue
    {
        let depth = llm_queue.depth(&db)?;
        if depth > 0 {
            llm::require_api_key(llm::MODEL)?;
        }
        let mut processed = 0;

        println!("Processing {} projects in LLM queue", depth);
        // process everything currently in the queue
        while processed < depth {
            if let Some(mut message) = llm_queue.pop(&mut db)? {
                let project = &message.payload;

                match project_to_tweet(project).await {
                    Ok(t) => {
                        eprintln!("Generated LLM tweet: {}", t);
                        let summarized = SummarizedProject {
                            project: project.clone(),
                            tweet: t,
                        };
                        slack_queue.push(&db, summarized.clone())?;
                        bsky_queue.push(&db, summarized)?;
                    }
                    Err(e) => {
                        message.attempts += 1;
                        let dead_lettered = message.attempts >= MAX_MESSAGE_PROCESSING_ATTEMPTS;
                        monitoring.error(
                            "Failed to summarize project",
                            &e,
                            &[
                                ("queue", json!("llm_queue")),
                                ("attempt", json!(message.attempts)),
                                ("max_attempts", json!(MAX_MESSAGE_PROCESSING_ATTEMPTS)),
                                ("dead_lettered", json!(dead_lettered)),
                                ("project_id", json!(project.id)),
                            ],
                        );
                        processing_failures += 1;

                        if message.attempts < MAX_MESSAGE_PROCESSING_ATTEMPTS {
                            llm_queue.push_message(&db, &message)?;
                        } else {
                            eprintln!(
                                "Message failed {} times; moving to dead letter queue",
                                message.attempts
                            );
                            llm_queue.push_to_dead_letter(&db, &message, &e.to_string())?;
                        }
                    }
                }
            }
            processed += 1;
        }
    }

    // Post to Slack if configured
    if let Some(ref webhook_url) = args.slack_webhook_url {
        processing_failures +=
            process_slack_queue(monitoring, &slack_queue, &mut db, webhook_url).await?;
    }

    // Post to Bluesky if configured
    if let (Some(user), Some(pass)) = (&args.bluesky_user, &args.bluesky_password) {
        let depth = bsky_queue.depth(&db)?;
        let mut processed = 0;
        println!("Processing {} tweets in Bluesky post queue", depth);

        // process everything currently in the queue
        while processed < depth {
            if let Some(mut message) = bsky_queue.pop(&mut db)? {
                if let Err(e) = bluesky::post_to_bluesky(
                    &message.payload.project,
                    &message.payload.tweet,
                    user,
                    pass,
                )
                .await
                {
                    message.attempts += 1;
                    let dead_lettered = message.attempts >= MAX_MESSAGE_PROCESSING_ATTEMPTS;
                    monitoring.error(
                        "Failed to post project to Bluesky",
                        &e,
                        &[
                            ("queue", json!("bluesky_post_queue")),
                            ("attempt", json!(message.attempts)),
                            ("max_attempts", json!(MAX_MESSAGE_PROCESSING_ATTEMPTS)),
                            ("dead_lettered", json!(dead_lettered)),
                            ("project_id", json!(message.payload.project.id)),
                        ],
                    );
                    processing_failures += 1;

                    if message.attempts < MAX_MESSAGE_PROCESSING_ATTEMPTS {
                        bsky_queue.push_message(&db, &message)?;
                    } else {
                        eprintln!(
                            "Message failed {} times; moving to dead letter queue",
                            message.attempts
                        );
                        bsky_queue.push_to_dead_letter(&db, &message, &e.to_string())?;
                    }
                }

                // Rate limit to avoid hitting the API too hard
                sleep(Duration::from_secs(1)).await;
            }
            processed += 1;
        }
    }

    // Conditions failures cannot block ordinary posts. Standalone tracking/summary
    // commands returned earlier and never enter this automatic posting path.
    if !args.skip_update_db {
        if let Err(error) = process_conditions(&args, &mut db, &latest_projects, monitoring).await {
            monitoring.error("Conditions pipeline failed", &error, &[]);
            processing_failures += 1;
        }
    }
    report_queue_depths(monitoring, &db, &llm_queue, &slack_queue, &bsky_queue)?;

    if processing_failures > 0 {
        return Err(anyhow!(
            "run completed with {processing_failures} processing failure(s)"
        ));
    }

    Ok(())
}

async fn process_conditions(
    args: &Args,
    db: &mut Database,
    projects: &[Project],
    monitoring: &Monitoring,
) -> Result<()> {
    let slack = args.slack_webhook_url.is_some();
    let bluesky = args.bluesky_user.is_some() && args.bluesky_password.is_some();
    let enabled = args.post_conditions && (slack || bluesky);
    if !enabled && !args.track_approvals {
        return Ok(());
    }
    let mut failures = Vec::new();
    if let Err(error) = approvals::track_for_posting(db, projects).await {
        failures.push(format!("Approval tracking: {error:#}"));
    }
    println!(
        "Approval storage: {}",
        serde_json::to_string(&approval_storage::stats(db)?)?
    );
    if enabled {
        match approval_posts::prepare(db, args.conditions_post_limit, Utc::now().timestamp()).await
        {
            Ok(errors) => failures.extend(errors),
            Err(error) => failures.push(format!("Conditions summaries: {error:#}")),
        }
        for channel in ["slack", "bluesky"] {
            if (channel == "slack" && !slack) || (channel == "bluesky" && !bluesky) {
                continue;
            }
            for _ in 0..args.conditions_post_limit {
                let Some(delivery) = approval_posts::claim(db, channel, Utc::now().timestamp())?
                else {
                    break;
                };
                let outcome = if channel == "slack" {
                    approval_posts::send_slack(
                        args.slack_webhook_url.as_deref().unwrap(),
                        &delivery.text,
                    )
                    .await
                } else {
                    match tokio::time::timeout(
                        Duration::from_secs(120),
                        bluesky::post_conditions(
                            &delivery,
                            args.bluesky_user.as_deref().unwrap(),
                            args.bluesky_password.as_deref().unwrap(),
                        ),
                    )
                    .await
                    {
                        Ok(Ok(())) => approval_posts::Outcome::Sent,
                        Ok(Err(_)) => approval_posts::Outcome::Retry(
                            "Bluesky conditions delivery failed".into(),
                        ),
                        Err(_) => approval_posts::Outcome::Retry(
                            "Bluesky conditions delivery timed out".into(),
                        ),
                    }
                };
                approval_posts::finish(db, delivery.id, channel, &outcome)?;
                if !matches!(outcome, approval_posts::Outcome::Sent) {
                    failures.push(format!(
                        "Conditions post {} ({channel}): {outcome:?}",
                        delivery.id
                    ));
                }
            }
        }
    }
    let counts = approval_posts::status_counts(db)?;
    println!("Conditions deliveries: {counts}");
    if counts.as_object().is_some_and(|counts| {
        counts.iter().any(|(key, value)| {
            (key.ends_with("_uncertain") || key.ends_with("_failed"))
                && value.as_i64().unwrap_or(0) > 0
        })
    }) {
        failures.push(format!("Conditions outbox needs attention: {counts}"));
    }
    let stats = approval_storage::stats(db)?;
    monitoring.gauge("rezoning_scraper.approval_pdf_bytes", stats.pdf_bytes, &[])?;
    if !failures.is_empty() {
        anyhow::bail!("{}", failures.join("; "));
    }
    Ok(())
}

async fn process_slack_queue(
    monitoring: &Monitoring,
    slack_queue: &Queue<SummarizedProject>,
    db: &mut Database,
    webhook_url: &str,
) -> Result<usize> {
    let depth = slack_queue.depth(db)?;
    let mut processed = 0;
    let mut failures = 0;

    // process everything currently in the queue
    while processed < depth {
        if let Some(mut message) = slack_queue.pop(db)? {
            let slack_message = create_slack_message(&message.payload);
            if let Err(e) = post_to_slack(webhook_url, slack_message).await {
                message.attempts += 1;
                message.last_attempt = Some(Utc::now());
                let dead_lettered = message.attempts >= MAX_MESSAGE_PROCESSING_ATTEMPTS;
                monitoring.error(
                    "Failed to post project to Slack",
                    &e,
                    &[
                        ("queue", json!("slack_post_queue")),
                        ("attempt", json!(message.attempts)),
                        ("max_attempts", json!(MAX_MESSAGE_PROCESSING_ATTEMPTS)),
                        ("dead_lettered", json!(dead_lettered)),
                        ("project_id", json!(message.payload.project.id)),
                    ],
                );
                failures += 1;

                if message.attempts < MAX_MESSAGE_PROCESSING_ATTEMPTS {
                    slack_queue.push_message(db, &message)?;
                } else {
                    eprintln!("Message failed, moving to dead letter queue: {e}");
                    slack_queue.push_to_dead_letter(db, &message, &e.to_string())?;
                }
            }
        }
        processed += 1;
    }

    Ok(failures)
}

fn report_queue_depths(
    monitoring: &Monitoring,
    db: &Database,
    llm_queue: &Queue<Project>,
    slack_queue: &Queue<SummarizedProject>,
    bsky_queue: &Queue<SummarizedProject>,
) -> Result<()> {
    for (name, depth, dead_letter_depth) in [
        (
            "llm_queue",
            llm_queue.depth(db)?,
            llm_queue.dead_letter_depth(db)?,
        ),
        (
            "slack_post_queue",
            slack_queue.depth(db)?,
            slack_queue.dead_letter_depth(db)?,
        ),
        (
            "bluesky_post_queue",
            bsky_queue.depth(db)?,
            bsky_queue.dead_letter_depth(db)?,
        ),
    ] {
        monitoring.gauge("rezoning_scraper.queue.depth", depth, &[("queue", name)])?;
        monitoring.gauge(
            "rezoning_scraper.dead_letter.depth",
            dead_letter_depth,
            &[("queue", name)],
        )?;
    }

    Ok(())
}

fn print_projects(
    new_projects: &Vec<Project>,
    _changed_projects: &[(Project, Vec<ProjectChange>)],
) {
    // Print results
    if !new_projects.is_empty() {
        println!("\n{}", "New Projects:".bold().green());
        for project in new_projects {
            println!("\n{}", project.attributes.name.bold());
            println!("State: {}", project.attributes.state.cyan());
            if !project.attributes.project_tag_list.is_empty() {
                println!(
                    "Tags: {}",
                    project.attributes.project_tag_list.join(", ").magenta()
                );
            }
            println!("URL: {}", project.links.self_link.blue().underline());
        }
    }

    // if !changed_projects.is_empty() {
    //     println!("\n{}", "Changed Projects:".bold().yellow());
    //     for (project, changes) in changed_projects {
    //         println!("\n{}", project.attributes.name.bold());
    //         for change in changes {
    //             println!(
    //                 "{}: '{}' -> '{}'",
    //                 change.field.green(),
    //                 change.old_value.red(),
    //                 change.new_value.green()
    //             );
    //         }
    //     }
    // }
}

async fn post_to_slack(webhook_url: &str, message: String) -> Result<()> {
    println!("{}", "Posting to Slack...".bold().cyan());
    let client = reqwest::Client::new();

    client
        .post(webhook_url)
        .header("Content-Type", "application/json")
        .body(message)
        .send()
        .await?
        .error_for_status()?;

    println!("{}", "Posted message to Slack".green());
    Ok(())
}

fn create_slack_message(project: &SummarizedProject) -> String {
    let mut message = String::new();

    let SummarizedProject { project, tweet } = project;

    message.push_str(&format!(
        "*<{}|{}>*\n",
        project.links.self_link,
        project.attributes.name.replace('\n', "")
    ));

    message.push_str(tweet);

    let mut json = serde_json::json!({
        "text": message
    });

    let image_url = project
        .attributes
        .image_url
        .as_deref()
        .map(str::trim)
        .filter(|url| {
            !url.is_empty()
                && !url.to_lowercase().contains("generic")
                && url.chars().count() <= 3000
                && reqwest::Url::parse(url)
                    .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
        });

    // Keep oversized messages text-only rather than exceeding Slack's section limit.
    if let Some(image_url) = image_url.filter(|_| message.chars().count() <= 3000) {
        let alt_text: String = project
            .attributes
            .image_description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .unwrap_or("Image from ShapeYourCity API")
            .chars()
            .take(2000)
            .collect();

        // Slack fetches the public image directly; no upload or additional hosting needed.
        // Retain top-level text for notifications and screen readers.
        json["blocks"] = json!([
            {
                "type": "section",
                "text": { "type": "mrkdwn", "text": message }
            },
            {
                "type": "image",
                "image_url": image_url,
                "alt_text": alt_text
            }
        ]);
        json["unfurl_links"] = json!(false);
        json["unfurl_media"] = json!(false);
    }

    json.to_string()
}

#[allow(dead_code)]
#[derive(Debug)]
struct ProjectChange {
    field: String,
    old_value: String,
    new_value: String,
}

// tests
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn slack_project() -> SummarizedProject {
        let projects: Projects =
            serde_json::from_str(include_str!("../test_files/ExampleInput.json")).unwrap();
        SummarizedProject {
            project: projects.data.into_iter().nth(1).unwrap(),
            tweet: "A five-storey apartment building is proposed.".to_string(),
        }
    }

    #[test]
    fn slack_includes_project_image_and_preserves_message_text() {
        let project = slack_project();
        let message: Value = serde_json::from_str(&create_slack_message(&project)).unwrap();
        let expected_text = format!(
            "*<{}|{}>*\n{}",
            project.project.links.self_link, project.project.attributes.name, project.tweet
        );

        assert_eq!(message["text"], expected_text);
        assert_eq!(message["blocks"][0]["text"]["text"], expected_text);
        assert_eq!(message["blocks"][0]["text"]["type"], "mrkdwn");
        assert_eq!(message["blocks"][1]["type"], "image");
        assert_eq!(
            message["blocks"][1]["image_url"],
            project.project.attributes.image_url.unwrap()
        );
        assert_eq!(
            message["blocks"][1]["alt_text"],
            "Example of a 5 storey apartment building"
        );
        assert_eq!(message["unfurl_links"], false);
        assert_eq!(message["unfurl_media"], false);
    }

    #[test]
    fn slack_keeps_posts_without_suitable_images_text_only() {
        let mut project = slack_project();
        for url in [
            None,
            Some(""),
            Some(" \n "),
            Some("https://example.com/GeNeRiC.jpg"),
            Some("not a URL"),
            Some("file:///image.jpg"),
        ] {
            project.project.attributes.image_url = url.map(str::to_string);
            let message: Value = serde_json::from_str(&create_slack_message(&project)).unwrap();
            assert!(
                message.get("blocks").is_none(),
                "unexpected image for {url:?}"
            );
            assert!(message["text"].as_str().unwrap().contains(&project.tweet));
        }
    }

    #[test]
    fn slack_image_alt_text_is_nonempty_and_within_slack_limits() {
        let mut project = slack_project();
        for description in [None, Some(""), Some(" \n ")] {
            project.project.attributes.image_description = description.map(str::to_string);
            let message: Value = serde_json::from_str(&create_slack_message(&project)).unwrap();
            assert_eq!(
                message["blocks"][1]["alt_text"],
                "Image from ShapeYourCity API"
            );
        }

        project.project.attributes.image_description = Some("🏠".repeat(2001));
        let message: Value = serde_json::from_str(&create_slack_message(&project)).unwrap();
        assert_eq!(message["blocks"][1]["alt_text"], "🏠".repeat(2000));
    }

    #[test]
    fn slack_keeps_oversized_messages_and_image_urls_text_only() {
        let mut project = slack_project();
        project.project.attributes.image_url =
            Some(format!("https://example.com/{}.jpg", "a".repeat(3000)));
        let message: Value = serde_json::from_str(&create_slack_message(&project)).unwrap();
        assert!(message.get("blocks").is_none());

        project.project.attributes.image_url = Some("https://example.com/image.jpg".to_string());
        project.tweet = "a".repeat(3001);
        let message: Value = serde_json::from_str(&create_slack_message(&project)).unwrap();
        assert!(message.get("blocks").is_none());
        assert!(message["text"].as_str().unwrap().ends_with(&project.tweet));
    }

    #[test]
    fn can_deserialize() {
        let json = include_str!("../test_files/ExampleInput.json");
        let result = serde_json::from_str::<Projects>(json).expect("Should deserialize");

        assert_eq!(result.data.len(), 30);
    }

    /* { "data": {
           "user_id": 467419949,
           "user_type": "AnonymousUser"
         },
         "exp": 1638294321,
         "iat": 1638121521,
         "iss": "Bang The Table Pvt Ltd",
         "jti": "49b984aa47751d338fc3baf7897e9685"
    }
    */
}
