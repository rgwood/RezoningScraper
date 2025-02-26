use anyhow::{anyhow, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;
use colored::Colorize;
use db::{Database, Token};
use indicatif::ProgressBar;
use models::{Project, Projects, SummarizedProject};
use queue::Queue;
use scraper::{Html, Selector};
use sentry::integrations::anyhow::capture_anyhow;
use serde_json::Value;
use std::time::Duration;
use summarizer::project_to_tweet;
use tokio::time::sleep;

mod bluesky;
mod db;
mod models;
mod queue;
mod summarizer;

const MAX_MESSAGE_PROCESSING_ATTEMPTS: i32 = 3;

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

    #[arg(
        long,
        help = "Use cached API responses (up to 1 hour old) when available"
    )]
    api_cache: bool,

    #[arg(long, help = "Skip updating the local database (useful for testing)")]
    skip_update_db: bool,
}

fn main() -> Result<()> {
    // Sentry needs to initialized before the tokio runtime
    let _guard = sentry::init((
        "https://b9aa3a714ea10fe4c30c0905fbc8db11@sentry-intake.datadoghq.com/1",
        sentry::ClientOptions {
            release: sentry::release_name!(),
            attach_stacktrace: true,
            ..Default::default()
        },
    ));

    if let Err(e) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
    {
        capture_anyhow(&e);
    }

    Ok(())
}

async fn async_main() -> Result<()> {
    let args = Args::parse();

    println!(
        "{}",
        format!("Rezoning Scraper v{}", env!("CARGO_PKG_VERSION"))
            .bold()
            .green()
    );

    if args.slack_webhook_url.is_none() {
        eprintln!(
            "{}",
            "Slack URI not specified; will not publish updates to Slack.".yellow()
        );
    }

    if args.bluesky_user.is_none() || args.bluesky_password.is_none() {
        eprintln!("Bluesky username and password are required; will not post to Bluesky.");
    }

    let mut db = Database::new_from_file("rezoning_scraper.db")?;

    println!("{}", "Getting API token...".bold().cyan());
    let token_spinner = ProgressBar::new_spinner();
    token_spinner.set_message("Getting API token...");
    token_spinner.enable_steady_tick(Duration::from_millis(100));
    let token = get_token_from_db_or_website(&mut db, &token_spinner).await?;

    println!("{}", "Querying API...".bold().cyan());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;

    // Fetch projects
    let start = std::time::Instant::now();
    let latest_projects = fetch_all_projects(&client, &token.jwt, &db, args.api_cache).await?;

    println!(
        "Retrieved {} projects in {}ms",
        format!("{}", latest_projects.len()).green(),
        format!("{}", start.elapsed().as_millis()).green()
    );

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

    // Process LLM queue
    {
        let depth = llm_queue.depth(&db)?;
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
                        eprintln!("Error processing project: {}", e);
                        message.attempts += 1;
                        if message.attempts < MAX_MESSAGE_PROCESSING_ATTEMPTS {
                            llm_queue.push_message(&db, &message)?;
                        } else {
                            eprintln!(
                                "Message failed {} times; moving to dead letter queue",
                                message.attempts
                            );
                            llm_queue.push_to_dead_letter(&db, &message, &e.to_string())?;
                            capture_anyhow(&e.context(
                                "Failed to summarize project, moving to dead letter queue",
                            ));
                        }
                    }
                }
            }
            processed += 1;
        }
    }

    // Post to Slack if configured
    if let Some(webhook_url) = args.slack_webhook_url {
        process_slack_queue(&slack_queue, &mut db, &webhook_url).await?;
    }

    // Post to Bluesky if configured
    if let (Some(user), Some(pass)) = (args.bluesky_user, args.bluesky_password) {
        let depth = bsky_queue.depth(&db)?;
        let mut processed = 0;
        println!("Processing {} tweets in Bluesky post queue", depth);

        // process everything currently in the queue
        while processed < depth {
            if let Some(mut message) = bsky_queue.pop(&mut db)? {
                if let Err(e) = bluesky::post_to_bluesky(
                    &message.payload.project,
                    &message.payload.tweet,
                    &user,
                    &pass,
                )
                .await
                {
                    eprintln!("Error posting to Bluesky: {}", e);
                    message.attempts += 1;
                    if message.attempts < MAX_MESSAGE_PROCESSING_ATTEMPTS {
                        bsky_queue.push_message(&db, &message)?;
                    } else {
                        eprintln!(
                            "Message failed {} times; moving to dead letter queue",
                            message.attempts
                        );
                        bsky_queue.push_to_dead_letter(&db, &message, &e.to_string())?;
                        capture_anyhow(
                            &e.context("Failed to post to Bluesky, moving to dead letter queue"),
                        );
                    }
                }

                // Rate limit to avoid hitting the API too hard
                sleep(Duration::from_secs(1)).await;
            }
            processed += 1;
        }
    }

    Ok(())
}

async fn process_slack_queue(
    slack_queue: &Queue<SummarizedProject>,
    db: &mut Database,
    webhook_url: &str,
) -> Result<()> {
    let depth = slack_queue.depth(db)?;
    let mut processed = 0;

    // process everything currently in the queue
    while processed < depth {
        if let Some(mut message) = slack_queue.pop(db)? {
            let slack_message = create_slack_message(&message.payload);
            if let Err(e) = post_to_slack(webhook_url, slack_message).await {
                message.attempts += 1;
                message.last_attempt = Some(Utc::now());
                eprintln!("Error posting to Slack: {}", e);
                if message.attempts < MAX_MESSAGE_PROCESSING_ATTEMPTS {
                    slack_queue.push_message(db, &message)?;
                } else {
                    eprintln!("Message failed,  moving to dead letter queue: {}", &e);
                    slack_queue.push_to_dead_letter(db, &message, &e.to_string())?;
                    capture_anyhow(
                        &e.context("Failed to post to Slack, moving to dead letter queue"),
                    );
                }
            }
        }
        processed += 1;
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

async fn get_token_from_db_or_website(db: &mut Database, pb: &ProgressBar) -> Result<Token> {
    // Check if we have a valid token in the DB
    if let Some(token) = db.get_token()? {
        let now = Utc::now();
        if token.expiration > now + chrono::Duration::minutes(1) {
            pb.finish_with_message(format!(
                "Loaded API token from database. Cached token will expire on {}",
                token.expiration.to_string().green()
            ));

            return Ok(token);
        }
    }

    pb.set_message("Getting latest anonymous user token from shapeyourcity.ca");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;

    let html = client
        .get("https://shapeyourcity.ca/embeds/projectfinder")
        .send()
        .await?
        .text()
        .await?;

    let jwt = extract_token_from_html(&html)?;
    let expiration = get_expiration_from_encoded_jwt(&jwt)?;

    pb.finish_with_message(format!(
        "Retrieved API token from website. Token will expire on {}",
        expiration.to_string().green()
    ));

    let token = Token { expiration, jwt };

    db.set_token(&token)?;

    Ok(token)
}

async fn fetch_all_projects(
    client: &reqwest::Client,
    jwt: &str,
    db: &Database,
    use_cache: bool,
) -> Result<Vec<Project>> {
    const RESULTS_PER_PAGE: u32 = 200;
    let mut all_projects = Vec::new();
    let mut next_url = Some(format!(
        "https://shapeyourcity.ca/api/v2/projects?per_page={}",
        RESULTS_PER_PAGE
    ));
    let mut page_count = 0;

    while let Some(url) = next_url {
        let pb = ProgressBar::new_spinner();
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_message(format!("Fetching page {}...", page_count + 1));

        let (response, used_cache): (Projects, bool) = if use_cache {
            if let Some(cached) = db.get_cached_response(&url)? {
                (serde_json::from_str(&cached)?, true)
            } else {
                let response = client
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", jwt))
                    .send()
                    .await?
                    .text()
                    .await?;

                db.cache_response(&url, &response)?;
                (serde_json::from_str(&response)?, false)
            }
        } else {
            let response = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", jwt))
                .send()
                .await?
                .text()
                .await?;

            db.cache_response(&url, &response)?;
            (serde_json::from_str(&response)?, false)
        };

        page_count += 1;

        if used_cache {
            pb.finish_with_message(format!(
                "Retrieved page {} from cache ({} items)",
                page_count,
                response.data.len()
            ));
        } else {
            pb.finish_with_message(format!(
                "Retrieved page {} ({} items)",
                page_count,
                response.data.len()
            ));
        }

        all_projects.extend(response.data);
        next_url = response.links.next;
    }

    Ok(all_projects)
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

    let json = serde_json::json!({
        "text": message
    });

    json.to_string()
}

#[allow(dead_code)]
#[derive(Debug)]
struct ProjectChange {
    field: String,
    old_value: String,
    new_value: String,
}

fn extract_token_from_html(html: &str) -> Result<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("script#__NEXT_DATA__").unwrap();

    let script = document
        .select(&selector)
        .next()
        .ok_or_else(|| anyhow!("Could not find NEXT_DATA tag"))?;

    let json: Value = serde_json::from_str(&script.inner_html())
        .map_err(|e| anyhow!("Could not parse NEXT_DATA JSON: {}", e))?;

    // Navigate the JSON structure to get the token
    json["props"]["pageProps"]["initialState"]["anonymousUser"]["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Could not find token in JSON"))
        .map(String::from)
}

fn get_expiration_from_encoded_jwt(jwt: &str) -> Result<DateTime<Utc>> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return Err(anyhow!("Invalid JWT format"));
    }

    // Decode the payload (middle part)
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| anyhow!("Failed to decode JWT payload: {}", e))?;

    let json: Value =
        serde_json::from_slice(&payload).map_err(|e| anyhow!("Failed to parse JWT JSON: {}", e))?;

    // Extract exp claim
    let exp = json["exp"]
        .as_i64()
        .ok_or_else(|| anyhow!("No expiration in JWT"))?;

    Utc.timestamp_opt(exp, 0)
        .single()
        .ok_or_else(|| anyhow!("Invalid timestamp"))
}

// tests
#[cfg(test)]
mod tests {
    use super::*;

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
    const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJpYXQiOjE2MzgxMjE1MjEsImp0aSI6IjQ5Yjk4NGFhNDc3NTFkMzM4ZmMzYmFmNzg5N2U5Njg1IiwiZXhwIjoxNjM4Mjk0MzIxLCJpc3MiOiJCYW5nIFRoZSBUYWJsZSBQdnQgTHRkIiwiZGF0YSI6eyJ1c2VyX2lkIjo0Njc0MTk5NDksInVzZXJfdHlwZSI6IkFub255bW91c1VzZXIifX0.p7FGWkT_7sWtC4gAsQ_HgaX-Z8aw88QQiGdHITmQleQ";
    const EXPIRATION_IN_UNIX_SECONDS: i64 = 1638294321;

    #[test]
    fn test_extract_jwt() {
        let html = include_str!("../test_files/projectFinder.html");
        let jwt = extract_token_from_html(html).expect("Should extract JWT");
        assert_eq!(jwt, JWT);
    }

    #[test]
    fn test_extract_expiration_from_jwt() {
        let expiration = get_expiration_from_encoded_jwt(JWT).expect("Should parse expiration");
        assert_eq!(expiration.timestamp(), EXPIRATION_IN_UNIX_SECONDS);
    }
}
