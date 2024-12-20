use std::vec;

use anyhow::Result;
use atrium_api::types::Union;
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

    let mut embed = None;

    if let Some(img_url) = &project.attributes.image_url {
        // sometimes they post generic images that we don't want to repost
        if !img_url.to_lowercase().contains("generic") {
            let img = reqwest::get(img_url).await?.bytes().await?;
            eprintln!("Downloaded image: {}", img_url);

            let output = agent.api.com.atproto.repo.upload_blob(img.to_vec()).await?;
            eprintln!("Uploaded image");

            let image = atrium_api::app::bsky::embed::images::ImageData {
                alt: "Project image".to_string(),
                aspect_ratio: None,
                image: output.data.blob,
            }
            .into();

            let images = vec![image];

            embed = Some(Union::Refs(
                atrium_api::app::bsky::feed::post::RecordEmbedRefs::AppBskyEmbedImagesMain(
                    Box::new(atrium_api::app::bsky::embed::images::MainData { images }.into()),
                ),
            ));
        }
    }

    let tweet_with_link = format!("{} {}", tweet_text, project.links.self_link);

    eprintln!("Tweeting: {}", tweet_with_link);

    let rt = RichText::new_with_detect_facets(tweet_with_link).await?;

    agent
        .create_record(atrium_api::app::bsky::feed::post::RecordData {
            created_at: atrium_api::types::string::Datetime::now(),
            embed,
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
