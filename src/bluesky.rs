use std::{num::NonZero, vec};

use anyhow::{bail, Context, Result};
use bsky_sdk::{
    api::{
        app::bsky::{
            embed::{
                defs::AspectRatioData,
                images::{self, ImageData},
            },
            feed::post::RecordEmbedRefs,
        },
        types::Union,
    },
    rich_text::RichText,
    BskyAgent,
};
use image::codecs::jpeg::JpegEncoder;

use crate::models::Project;

// Hard limit on image size to post to Bluesky
const MAX_IMAGE_SIZE_BYTES: usize = 1_000_000;

/// A stable record key and read-back let interrupted requests retry without duplicates.
pub async fn post_conditions(
    delivery: &crate::approval_posts::Delivery,
    username: &str,
    password: &str,
) -> Result<()> {
    let agent = BskyAgent::builder().build().await?;
    agent.login(username, password).await?;
    write_conditions(&agent, delivery).await
}

async fn write_conditions(
    agent: &BskyAgent,
    delivery: &crate::approval_posts::Delivery,
) -> Result<()> {
    use bsky_sdk::api::com::atproto::repo::{create_record, get_record};
    let mut value = serde_json::to_value(conditions_record(delivery)?)?;
    value["$type"] = serde_json::json!("app.bsky.feed.post");
    let record: bsky_sdk::api::types::Unknown = serde_json::from_value(value)?;
    let repo = agent.did().await.context("Bluesky login missing")?.into();
    let rkey = delivery.record_key.parse().map_err(anyhow::Error::msg)?;
    let collection = "app.bsky.feed.post".parse().map_err(anyhow::Error::msg)?;
    let request = create_record::InputData {
        collection,
        record: record.clone(),
        repo,
        rkey: Some(rkey),
        swap_commit: None,
        validate: Some(true),
    };
    if let Err(error) = agent
        .api
        .com
        .atproto
        .repo
        .create_record(request.clone().into())
        .await
    {
        // A previous request may have succeeded before the connection dropped.
        // Verify the exact record; never overwrite a colliding or edited post.
        let existing = agent
            .api
            .com
            .atproto
            .repo
            .get_record(
                get_record::ParametersData {
                    cid: None,
                    collection: request.collection,
                    repo: request.repo,
                    rkey: request.rkey.unwrap(),
                }
                .into(),
            )
            .await
            .with_context(|| format!("Bluesky write failed and could not be verified: {error}"))?;
        anyhow::ensure!(
            existing.data.value == record,
            "Bluesky record key already contains a different post"
        );
    }
    Ok(())
}

fn conditions_record(
    delivery: &crate::approval_posts::Delivery,
) -> Result<bsky_sdk::api::app::bsky::feed::post::RecordData> {
    anyhow::ensure!(
        delivery.text.chars().count() <= 300,
        "Conditions post is too long"
    );
    let (body, url) = delivery
        .text
        .rsplit_once('\n')
        .context("Missing conditions link")?;
    let parsed = reqwest::Url::parse(url)?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "https" | "http"),
        "Invalid conditions link"
    );
    let created_at = chrono::DateTime::from_timestamp(delivery.created_at, 0)
        .context("Invalid post timestamp")?
        .to_rfc3339();
    // Link offsets are UTF-8 bytes, not character counts. No network facet discovery needed.
    Ok(serde_json::from_value(serde_json::json!({
        "text":delivery.text,"createdAt":created_at,
        "facets":[{"index":{"byteStart":body.len()+1,"byteEnd":delivery.text.len()},
        "features":[{"$type":"app.bsky.richtext.facet#link","uri":url}]}]
    }))?)
}

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

    let tags = &project.attributes.project_tag_list;
    let tweet_with_link = if tags.iter().any(|tag| tag == "Development") {
        format!("DP: {} {}", tweet_text, project.links.self_link)
    } else if tags.iter().any(|tag| tag == "Rezoning") {
        format!("Rezoning: {} {}", tweet_text, project.links.self_link)
    } else {
        format!("{} {}", tweet_text, project.links.self_link)
    };

    eprintln!("Tweeting: {}", tweet_with_link);

    let rt = RichText::new_with_detect_facets(tweet_with_link).await?;

    agent
        .create_record(bsky_sdk::api::app::bsky::feed::post::RecordData {
            created_at: bsky_sdk::api::types::string::Datetime::now(),
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

    #[tokio::test]
    async fn conditions_retry_reads_back_same_record_without_creating_a_duplicate() {
        use serde_json::{json, Value};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let delivery = crate::approval_posts::Delivery {
            id: 1,
            text: "Approval conditions for École: five bike spaces.\nhttps://example.com/letter"
                .into(),
            record_key: "3jzfcijpj2z2a".into(),
            created_at: 1_789_920_000,
        };
        let expected = delivery.text.clone();
        let server = tokio::spawn(async move {
            let mut saved = Value::Null;
            for step in 0..5 {
                let (mut socket, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut bytes = Vec::new();
                let (start, len) = loop {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..i]);
                        let len = headers
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_owned)
                            })
                            .map(|v| v.parse::<usize>().unwrap())
                            .unwrap_or(0);
                        break (i + 4, len);
                    }
                };
                while bytes.len() < start + len {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let headers = String::from_utf8_lossy(&bytes[..start]);
                let (status, body) = match step {
                    0 => {
                        assert!(headers.starts_with("POST /xrpc/com.atproto.server.createSession"));
                        (
                            200,
                            json!({"accessJwt":"fake-access","refreshJwt":"fake-refresh","did":"did:plc:abcdefghijklmnopqrstuvwx","handle":"example.test"}),
                        )
                    }
                    1 | 3 => {
                        assert!(headers.starts_with("POST /xrpc/com.atproto.repo.createRecord"));
                        let body: Value =
                            serde_json::from_slice(&bytes[start..start + len]).unwrap();
                        assert_eq!(body["rkey"], "3jzfcijpj2z2a");
                        assert_eq!(body["record"]["$type"], "app.bsky.feed.post");
                        assert_eq!(body["record"]["text"], expected);
                        assert_eq!(body["validate"], true);
                        if step == 1 {
                            saved = body["record"].clone();
                        } else {
                            assert_eq!(body["record"], saved);
                        }
                        (
                            400,
                            json!({"error":"InvalidRequest","message":"Record already exists"}),
                        )
                    }
                    2 | 4 => {
                        assert!(headers.starts_with("GET /xrpc/com.atproto.repo.getRecord?"));
                        assert!(headers.contains("rkey=3jzfcijpj2z2a"));
                        let mut value = saved.clone();
                        if step == 4 {
                            value["text"] = json!("Someone edited this post");
                        }
                        (
                            200,
                            json!({"uri":"at://did:plc:abcdefghijklmnopqrstuvwx/app.bsky.feed.post/3jzfcijpj2z2a","cid":"bafyreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","value":value}),
                        )
                    }
                    _ => unreachable!(),
                };
                let body = body.to_string();
                socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
        });
        let agent = BskyAgent::builder()
            .config(bsky_sdk::agent::config::Config {
                endpoint,
                ..Default::default()
            })
            .build()
            .await
            .unwrap();
        agent
            .login("example.test", "fake-test-password")
            .await
            .unwrap();
        write_conditions(&agent, &delivery).await.unwrap();
        // A colliding/edited record is held rather than overwritten.
        assert!(write_conditions(&agent, &delivery)
            .await
            .unwrap_err()
            .to_string()
            .contains("different post"));
        server.await.unwrap();
        let value = serde_json::to_value(conditions_record(&delivery).unwrap()).unwrap();
        let start = value["facets"][0]["index"]["byteStart"].as_u64().unwrap() as usize;
        assert_eq!(&delivery.text[start..], "https://example.com/letter");
        assert_eq!(value["facets"][0]["index"]["byteEnd"], delivery.text.len());
    }

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
