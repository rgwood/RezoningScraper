use std::{num::NonZero, vec};

use anyhow::{bail, Context, Result};
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
use image::codecs::jpeg::JpegEncoder;

use crate::models::Project;

// Hard limit on image size to post to Bluesky
const MAX_IMAGE_SIZE_BYTES: usize = 1_000_000;

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
        if !img_url.trim().is_empty() && !img_url.to_lowercase().contains("generic") {
            let img_bytes = reqwest::get(img_url).await?.bytes().await?;
            eprintln!("Downloaded image: {}", img_url);

            let img_bytes = compress_image_until_under_size(&img_bytes)?;

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

fn compress_image_until_under_size(img: &[u8]) -> Result<Vec<u8>> {
    if img.len() < MAX_IMAGE_SIZE_BYTES {
        return Ok(img.to_vec());
    }

    let img = image::load_from_memory(img)?;
    let mut buffer = vec![];
    let mut quality = 90;

    loop {
        buffer.clear();

        JpegEncoder::new_with_quality(&mut buffer, quality).encode_image(&img)?;

        eprintln!(
            "Resized image (quality: {}) size: {}b",
            quality,
            buffer.len()
        );

        if buffer.len() < MAX_IMAGE_SIZE_BYTES {
            return Ok(buffer);
        }

        quality = quality.saturating_sub(5);

        if quality == 0 {
            break;
        }
    }

    bail!(
        "Failed to compress image to under {}b",
        MAX_IMAGE_SIZE_BYTES
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn resize_image() {
        let img_bytes = include_bytes!("../test_files/too_big.jpg");

        eprintln!("Image size: {}b", img_bytes.len());

        assert!(img_bytes.len() > MAX_IMAGE_SIZE_BYTES);

        let buffer = compress_image_until_under_size(img_bytes).unwrap();
        eprintln!("Resized image size: {}b", buffer.len());

        // write buffer to disk as resized.jpg
        let mut file = std::fs::File::create("resized.jpg").unwrap();
        file.write_all(buffer.as_slice()).unwrap();

        // todo!()
    }
}
