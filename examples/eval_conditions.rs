//! Run the production conditions workflow against frozen fixtures, never posting.
use anyhow::{ensure, Context, Result};
use clap::Parser;
use rezoning_scraper::conditions::{
    analysis::{analyze_pages, require_api_key, workflow_fingerprint, Analysis, DEFAULT_MODEL},
    extract_pages, EXTRACTOR_VERSION, PROMPT_VERSION,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc, time::Instant};
use tokio::{sync::Semaphore, task::JoinSet};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "evals/conditions/cases.json")]
    cases: PathBuf,
    #[arg(long, default_value = DEFAULT_MODEL)]
    model: String,
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=20))]
    repeats: u32,
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=8))]
    parallel: u32,
    #[arg(long)]
    case: Vec<String>,
    #[arg(long)]
    split: Option<String>,
    #[arg(long)]
    extract_only: bool,
    #[arg(
        long,
        help = "Keep completed trials and retry only explicit credit-quota interruptions; preserve interrupted attempts"
    )]
    resume_quota_failures: bool,
}

#[derive(Clone, Deserialize)]
struct Case {
    id: String,
    project_name: String,
    source_url: String,
    pdf: Option<String>,
    pages: Option<Vec<String>>,
    split: String,
    sha256: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if !args.extract_only {
        require_api_key(&args.model)?;
    }
    let case_bytes = std::fs::read(&args.cases)?;
    let dataset_sha256 = format!("{:x}", Sha256::digest(&case_bytes));
    let cases: Vec<Case> = serde_json::from_slice(&case_bytes)?;
    let root = args
        .cases
        .parent()
        .context("Missing cases directory")?
        .to_path_buf();
    std::fs::create_dir_all(&args.output)?;
    let cases: Vec<_> = cases
        .into_iter()
        .filter(|c| {
            (args.case.is_empty() || args.case.contains(&c.id))
                && args.split.as_ref().is_none_or(|s| &c.split == s)
        })
        .collect();
    ensure!(!cases.is_empty(), "No matching evaluation cases");
    for id in &args.case {
        ensure!(
            cases.iter().any(|c| &c.id == id),
            "Unknown or excluded case: {id}"
        );
    }
    let mut seen = std::collections::HashSet::new();
    let mut completed = std::collections::HashSet::new();
    // Validate the entire run before dispatching any paid API request.
    for case in &cases {
        ensure!(seen.insert(&case.id), "Duplicate case ID: {}", case.id);
        if let Some(pdf) = &case.pdf {
            ensure!(case.pages.is_none(), "Case has both PDF and supplied pages");
            let actual = format!("{:x}", Sha256::digest(std::fs::read(root.join(pdf))?));
            ensure!(
                case.sha256.as_deref() == Some(&actual),
                "PDF fixture hash mismatch: {}",
                case.id
            );
        } else {
            ensure!(case.pages.is_some(), "Case needs PDF or pages: {}", case.id);
        }
        for repeat in 1..=args.repeats {
            let path = args.output.join(format!("{}-{repeat}.json", case.id));
            if path.exists() {
                ensure!(
                    args.resume_quota_failures && !args.extract_only,
                    "Refusing to overwrite {}",
                    path.display()
                );
                let previous: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
                ensure!(
                    previous["model"] == args.model
                        && previous["dataset_sha256"] == dataset_sha256
                        && previous["workflow_sha256"] == workflow_fingerprint(),
                    "Resume requires the same model, dataset and workflow"
                );
                if !previous["analysis"]["summary"].is_null() {
                    completed.insert((case.id.clone(), repeat));
                } else {
                    ensure!(
                        previous["analysis"]["error"]
                            .as_str()
                            .is_some_and(|e| e.contains("credit_balance_exhausted")),
                        "Only explicit credit-quota failures can be resumed: {}",
                        path.display()
                    );
                }
            }
        }
    }
    let semaphore = Arc::new(Semaphore::new(args.parallel as usize));
    let mut jobs = JoinSet::new();
    for case in cases {
        for repeat in 1..=args.repeats {
            if completed.contains(&(case.id.clone(), repeat)) {
                continue;
            }
            let case = case.clone();
            let path = args.output.join(format!("{}-{repeat}.json", case.id));
            let previous = if path.exists() {
                Some(serde_json::from_slice::<serde_json::Value>(
                    &std::fs::read(&path)?,
                )?)
            } else {
                None
            };
            let root = root.clone();
            let model = args.model.clone();
            let dataset_sha256 = dataset_sha256.clone();
            let semaphore = semaphore.clone();
            let extract_only = args.extract_only;
            jobs.spawn(async move {
                let _permit = semaphore.acquire_owned().await?;
                let start = Instant::now();
                let extracted = match case.pages {
                    Some(pages) => Ok(pages),
                    None => {
                        extract_pages(std::fs::read(
                            root.join(case.pdf.context("Case needs pdf or pages")?),
                        )?)
                        .await
                    }
                };
                let (pages, analysis) = match extracted {
                    Ok(pages) => {
                        let analysis = if extract_only {
                            None
                        } else {
                            Some(
                                analyze_pages(
                                    &genai::Client::default(),
                                    &model,
                                    &case.project_name,
                                    &case.source_url,
                                    &pages,
                                )
                                .await,
                            )
                        };
                        (pages, analysis)
                    }
                    Err(error) => (
                        vec![],
                        Some(Analysis {
                            workflow_sha256: workflow_fingerprint(),
                            summary: None,
                            error: Some(format!("{error:#}")),
                            trace: vec![],
                        }),
                    ),
                };
                let passed = analysis.as_ref().is_some_and(|a| a.summary.is_some())
                    || (extract_only && !pages.is_empty());
                let mut result = json!({"id":case.id, "repeat":repeat, "model":model,
                    "generated_at":chrono::Utc::now().to_rfc3339(),
                    "dataset_sha256":dataset_sha256,"workflow_sha256":workflow_fingerprint(),
                    "pipeline_version":PROMPT_VERSION,"extractor_version":EXTRACTOR_VERSION,
                    "project_name":case.project_name, "source_url":case.source_url,
                    "elapsed_ms":start.elapsed().as_millis(), "pages":pages, "analysis":analysis});
                if let Some(previous) = previous {
                    result["quota_interrupted_attempt"] = previous;
                }
                std::fs::write(path, serde_json::to_vec_pretty(&result)?)?;
                eprintln!(
                    "{} #{repeat}: {} ({}s)",
                    case.id,
                    if passed { "OK" } else { "FAILED" },
                    start.elapsed().as_secs()
                );
                Ok::<_, anyhow::Error>(passed)
            });
        }
    }
    let mut passed = completed.len();
    let mut total = completed.len();
    while let Some(result) = jobs.join_next().await {
        total += 1;
        passed += usize::from(result??);
    }
    println!("{passed}/{total} completed successfully. Editorial scoring is a separate step.");
    ensure!(
        passed == total,
        "Some evaluation runs failed; all results were preserved"
    );
    Ok(())
}
