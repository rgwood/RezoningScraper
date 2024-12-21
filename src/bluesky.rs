use std::{num::NonZero, vec};

use anyhow::{Context, Result};
use atrium_api::{
    app::bsky::{
        embed::{
            defs::AspectRatioData,
            images::{self, ImageData},
        },
        feed::post::RecordEmbedRefs,
    },
    types::Union,
};
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
            let img_bytes = reqwest::get(img_url).await?.bytes().await?;
            eprintln!("Downloaded image: {}", img_url);

            let img = image::load_from_memory(&img_bytes)?;
            let height = NonZero::new(img.height() as u64).context("Image height is zero")?;
            let width = NonZero::new(img.width() as u64).context("Image width is zero")?;
            let aspect_ratio = AspectRatioData { height, width };
            eprintln!("Calculated aspect ratio: {}x{}", width, height);

            let output = agent
                .api
                .com
                .atproto
                .repo
                .upload_blob(img_bytes.to_vec())
                .await?;
            eprintln!("Uploaded image");

            let image = ImageData {
                alt: project
                    .attributes
                    .image_description
                    .clone()
                    .unwrap_or("Image from ShapeYourCity API".to_string()),
                aspect_ratio: Some(aspect_ratio.into()),
                image: output.data.blob,
            }
            .into();

            let images = vec![image];

            embed = Some(Union::Refs(RecordEmbedRefs::AppBskyEmbedImagesMain(
                Box::new(images::MainData { images }.into()),
            )));
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
