use anyhow::Result;

use colored::Colorize;
use rezoning_scraper::{
    db::Database,
    summarizer::{self},
};

#[tokio::main]
async fn main() -> Result<()> {
    let db = Database::new_from_file("rezoning_scraper.db")?;

    let projects = db.get_projects()?.into_iter().rev().skip(22).take(3);

    for proj in projects {
        let description_md =
            summarizer::html_to_markdown(proj.attributes.description.as_ref().unwrap());

        let summary = summarizer::project_to_tweet(&proj).await?;

        println!("{}", "Original Description:".bold().green());
        println!("{}", proj.attributes.name);
        println!("{}\n", description_md);

        println!("{}", "Tweet:".bold().green());
        println!("{}\n", summary);
    }

    Ok(())
}
