//! Exercise ordinary project summaries using saved inputs; never opens a DB or posts.
use anyhow::{ensure, Result};
use clap::Parser;
use rezoning_scraper::{llm, models::Projects, summarizer};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Instant};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "test_files/ExampleInput.json")]
    projects: PathBuf,
    #[arg(long)]
    project_id: Vec<String>,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=50))]
    limit: u32,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=5))]
    repeats: u32,
    #[arg(long)]
    output: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    llm::require_api_key(llm::MODEL)?;
    let bytes = std::fs::read(&args.projects)?;
    let input_hash = format!("{:x}", Sha256::digest(&bytes));
    let projects: Projects = serde_json::from_slice(&bytes)?;
    let projects: Vec<_> = projects
        .data
        .into_iter()
        .filter(|p| {
            if args.project_id.is_empty() {
                p.attributes.name.contains("rezoning application")
                    || p.attributes.name.contains("development application")
            } else {
                args.project_id.contains(&p.id)
            }
        })
        .take(args.limit as usize)
        .collect();
    ensure!(!projects.is_empty(), "No matching projects");
    for id in &args.project_id {
        ensure!(
            projects.iter().any(|p| &p.id == id),
            "Unknown or excluded project {id}"
        );
    }
    std::fs::create_dir_all(&args.output)?;
    for project in &projects {
        for repeat in 1..=args.repeats {
            ensure!(
                !args
                    .output
                    .join(format!("{}-{repeat}.json", project.id))
                    .exists(),
                "Refusing to overwrite an existing trial"
            );
        }
    }
    let mut passed = 0;
    for project in &projects {
        for repeat in 1..=args.repeats {
            let started = Instant::now();
            let result = summarizer::summarize_project(project).await;
            let (summary, error) = match result {
                Ok(summary) => {
                    passed += 1;
                    (Some(summary), None)
                }
                Err(error) => (None, Some(format!("{error:#}"))),
            };
            let output = json!({"project_id":project.id,"repeat":repeat,"model":llm::MODEL,
                "input_sha256":input_hash,"generated_at":chrono::Utc::now().to_rfc3339(),
                "elapsed_ms":started.elapsed().as_millis(),"name":project.attributes.name,
                "source_url":project.links.self_link,"description":summarizer::html_to_markdown(project.attributes.description.as_deref().unwrap_or_default()),
                "summary":summary,"error":error});
            std::fs::write(
                args.output.join(format!("{}-{repeat}.json", project.id)),
                serde_json::to_vec_pretty(&output)?,
            )?;
            eprintln!(
                "{} #{repeat}: {}",
                project.id,
                if error.is_none() { "OK" } else { "FAILED" }
            );
        }
    }
    let total = projects.len() * args.repeats as usize;
    eprintln!(
        "{passed}/{total} generated; review facts against the saved descriptions separately."
    );
    ensure!(passed == total, "Some trials failed; results preserved");
    Ok(())
}
