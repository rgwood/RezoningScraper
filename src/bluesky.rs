use anyhow::Result;
use bsky_sdk::{rich_text::RichText, BskyAgent};

use crate::models::Project;

pub async fn post_to_bluesky(
    project: &Project,
    tweet_text: &str,
    username: &str,
    password: &str,
) -> Result<()> {
    let agent = BskyAgent::builder().build().await?;

    // TODO: persist the token?
    _ = agent.login(username, password).await?;

    // TODO: upload image

    let tweet_with_link = format!("{} {}", tweet_text, project.links.self_link);

    eprintln!("Tweeting: {}", tweet_with_link);

    let rt = RichText::new_with_detect_facets(tweet_with_link).await?;

    agent
        .create_record(atrium_api::app::bsky::feed::post::RecordData {
            created_at: atrium_api::types::string::Datetime::now(),
            embed: None,
            entities: None,
            facets: rt.facets,
            labels: None,
            langs: None,
            reply: None,
            tags: None,
            text: rt.text,
        })
        .await?;

    Ok(())
}
