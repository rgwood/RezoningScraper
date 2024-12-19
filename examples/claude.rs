use anyhow::Result;

use colored::Colorize;
use genai::chat::{ChatMessage, ChatRequest};
use rezoning_scraper::{db::Database, summarizer::html_to_markdown};

#[tokio::main]
async fn main() -> Result<()> {
    let db = Database::new_from_file("rezoning_scraper.db")?;

    let projects = db.get_projects()?.into_iter().rev().skip(7).take(5);

    for proj in projects {
        const MODEL_ANTHROPIC: &str = "claude-3-5-haiku-20241022";

        let mut user_message = r#"Take the following description of a project on Vancouver's public consultation website and summarize it for a tweet. Do not use hashtags. Be succinct; only include the most important information. Don't mention that the project is pending approval; the reader will know. If it's a rezoning application, preface the tweet with "Rezoning application". If it's a development application, preface it with "Development application:".\n"#.to_string();
        user_message += &format!("{}\n", proj.attributes.name.replace('\n', ""));
        let description_html = &proj.attributes.description.clone().unwrap_or_default();
        let description_md = html_to_markdown(description_html);
        user_message += &description_md;

        let chat_req = ChatRequest::new(vec![
            // ChatMessage::system(""),
            ChatMessage::user(user_message),
        ]);

        let client = genai::Client::default();
        println!("{}", "Original Description:".bold().green());
        println!("{}", proj.attributes.name);
        println!("{}\n", description_md);

        let chat_res = client
            .exec_chat(MODEL_ANTHROPIC, chat_req.clone(), None)
            .await?;

        println!("{}", "Tweet:".bold().green());
        println!(
            "{}\n",
            chat_res.content_text_as_str().unwrap_or("NO ANSWER")
        );
    }

    Ok(())
}
