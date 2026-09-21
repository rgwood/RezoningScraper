use anyhow::{bail, ensure, Context, Result};
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatResponse, StopReason};
use html2md::{TagHandler, TagHandlerFactory};
use std::collections::HashMap;

use crate::{llm, models::Project};

pub(crate) const MODEL: &str = llm::MODEL;

/// Convert HTML to Markdown, ignoring images and not including URLs
pub fn html_to_markdown(html: &str) -> String {
    let mut handlers = HashMap::<String, Box<dyn TagHandlerFactory>>::new();
    handlers.insert("img".to_string(), Box::new(IgnoreHandlerFactory));
    handlers.insert("a".to_string(), Box::new(TextOnlyHandlerFactory));

    html2md::parse_html_custom(html, &handlers)
}

pub async fn project_to_tweet(proj: &Project) -> Result<String> {
    Ok(summarize_project(proj).await?.text)
}

#[derive(Debug, serde::Serialize)]
pub struct ProjectSummary {
    pub text: String,
    pub attempts: Vec<serde_json::Value>,
}

pub async fn summarize_project(proj: &Project) -> Result<ProjectSummary> {
    llm::require_api_key(MODEL)?;
    project_to_tweet_with_client(proj, &genai::Client::default()).await
}

async fn project_to_tweet_with_client(
    proj: &Project,
    client: &genai::Client,
) -> Result<ProjectSummary> {
    let mut user_message = "Summarize this:\n".to_string();
    user_message += &format!("# {}\n", proj.attributes.name.replace('\n', ""));
    let description_html = &proj.attributes.description.clone().unwrap_or_default();
    let description_md = html_to_markdown(description_html);
    user_message += &description_md;

    let mut messages = vec![
        ChatMessage::system(include_str!("system_prompt.txt")),
        ChatMessage::user(user_message.clone()),
    ];
    let options = ChatOptions::default().with_capture_raw_body(true).with_max_tokens(2000).with_extra_body(
        serde_json::json!({"provider":llm::provider_options(MODEL)?,"reasoning":{"effort":"medium"}}),
    );
    let mut attempts = Vec::new();
    for attempt in 0..2 {
        let chat_res = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            client.exec_chat(MODEL, ChatRequest::new(messages.clone()), Some(&options)),
        )
        .await
        .context("Project summary request timed out")??;
        let draft = record_response(chat_res, "draft", &mut attempts)?;
        let review = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            client.exec_chat(
                MODEL,
                ChatRequest::new(vec![
                    ChatMessage::system(include_str!("application_review_prompt.txt")),
                    ChatMessage::user(
                        serde_json::json!({"source":user_message,"draft":draft}).to_string(),
                    ),
                ]),
                Some(&options),
            ),
        )
        .await
        .context("Project summary source check timed out")??;
        let response = record_response(review, "source_check", &mut attempts)?;
        if response.chars().count() <= 140 {
            return Ok(ProjectSummary {
                text: response,
                attempts,
            });
        }
        if attempt == 0 {
            messages.push(ChatMessage::assistant(response.clone()));
            messages.push(ChatMessage::user(format!("That is {} characters. Rewrite it in at most 140 characters, keeping the key project facts. Return only the post text.", response.chars().count())));
        }
    }
    bail!("Project summary still exceeds 140 characters after one rewrite")
}

fn record_response(
    response: ChatResponse,
    stage: &str,
    attempts: &mut Vec<serde_json::Value>,
) -> Result<String> {
    ensure!(
        matches!(response.stop_reason, Some(StopReason::Completed(_))),
        "Project summary response was incomplete: {:?}",
        response.stop_reason
    );
    let text = response
        .first_text()
        .context("Failed to get chat response")?
        .trim();
    ensure!(!text.is_empty(), "Project summary was empty");
    let raw = response.captured_raw_body.as_ref();
    attempts.push(
        serde_json::json!({"stage":stage,"text":text,"usage":response.usage,
        "provider":raw.map(|r| &r["provider"]),"model":raw.map(|r| &r["model"]),
        "generation_id":raw.map(|r| &r["id"]),"provider_usage":raw.map(|r| &r["usage"])}),
    );
    Ok(text.to_owned())
}

struct IgnoreHandlerFactory;
struct IgnoreHandler;

impl TagHandler for IgnoreHandler {
    fn handle(&mut self, _tag: &html2md::Handle, _printer: &mut html2md::StructuredPrinter) {}

    fn skip_descendants(&self) -> bool {
        true
    }

    fn after_handle(&mut self, _printer: &mut html2md::StructuredPrinter) {}
}

impl TagHandlerFactory for IgnoreHandlerFactory {
    fn instantiate(&self) -> Box<dyn TagHandler> {
        Box::new(IgnoreHandler)
    }
}

struct TextOnlyHandlerFactory;
struct TextOnlyHandler;

impl TagHandler for TextOnlyHandler {
    fn handle(&mut self, _tag: &html2md::Handle, _printer: &mut html2md::StructuredPrinter) {}

    fn after_handle(&mut self, _printer: &mut html2md::StructuredPrinter) {}
}

impl TagHandlerFactory for TextOnlyHandlerFactory {
    fn instantiate(&self) -> Box<dyn TagHandler> {
        Box::new(TextOnlyHandler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn fixture(development: bool) -> Project {
        let projects: crate::models::Projects =
            serde_json::from_str(include_str!("../test_files/ExampleInput.json")).unwrap();
        projects
            .data
            .into_iter()
            .find(|p| {
                p.attributes.name.contains(if development {
                    "development application"
                } else {
                    "rezoning application"
                })
            })
            .unwrap()
    }

    async fn mock_client(
        outputs: Vec<(&str, String)>,
    ) -> (genai::Client, tokio::task::JoinHandle<Vec<Value>>) {
        use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
        let outputs: Vec<_> = outputs
            .into_iter()
            .map(|(reason, text)| (reason.to_owned(), text))
            .collect();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (reason, text) in outputs {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (start, len) = loop {
                    let mut buffer = [0; 4096];
                    let read = socket.read(&mut buffer).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..index]);
                        assert!(headers.starts_with("POST /v1/chat/completions"));
                        let len: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_owned)
                            })
                            .unwrap()
                            .parse()
                            .unwrap();
                        break (index + 4, len);
                    }
                };
                while bytes.len() < start + len {
                    let mut buffer = [0; 4096];
                    let read = socket.read(&mut buffer).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&buffer[..read]);
                }
                let request: Value = serde_json::from_slice(&bytes[start..start + len]).unwrap();
                assert_eq!(request["model"], "z-ai/glm-5.3-flash");
                assert_eq!(request["provider"], llm::provider_options(MODEL).unwrap());
                assert_eq!(request["reasoning"]["effort"], "medium");
                assert_eq!(request["max_tokens"], 2000);
                assert!(request.get("models").is_none());
                let system = request["messages"][0]["content"].as_str().unwrap();
                assert!(
                    system == include_str!("system_prompt.txt")
                        || system == include_str!("application_review_prompt.txt")
                );
                requests.push(request);
                let body = json!({"id":"local-test","model":"z-ai/glm-5.3-flash","provider":"Z.AI","usage":{"prompt_tokens":20,"completion_tokens":30,"cost":0.0001},"choices":[{"finish_reason":reason,"message":{"role":"assistant","content":text}}]}).to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        let client = genai::Client::builder()
            .with_auth_resolver(AuthResolver::from_resolver_fn(|_| {
                Ok(Some(AuthData::from_single("fake-local-key")))
            }))
            .with_service_target_resolver(ServiceTargetResolver::from_resolver_fn(
                move |mut target: genai::ServiceTarget| {
                    target.endpoint = Endpoint::from_owned(endpoint.clone());
                    Ok(target)
                },
            ))
            .build();
        (client, server)
    }

    #[tokio::test]
    async fn both_project_types_use_official_zai_and_record_usage() {
        for development in [false, true] {
            let project = fixture(development);
            let (client, server) = mock_client(vec![
                ("stop", "An inaccurate first draft.".into()),
                ("stop", "  A concise project summary.  ".into()),
            ])
            .await;
            let summary = project_to_tweet_with_client(&project, &client)
                .await
                .unwrap();
            assert_eq!(summary.text, "A concise project summary.");
            assert_eq!(summary.attempts[0]["provider"], "Z.AI");
            assert_eq!(summary.attempts[0]["provider_usage"]["cost"], 0.0001);
            assert_eq!(summary.attempts[1]["stage"], "source_check");
            let requests = server.await.unwrap();
            assert!(requests[0]["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains(project.attributes.name.trim()));
            assert_eq!(
                requests[1]["messages"][0]["content"],
                include_str!("application_review_prompt.txt")
            );
            let review: Value =
                serde_json::from_str(requests[1]["messages"][1]["content"].as_str().unwrap())
                    .unwrap();
            assert_eq!(review["draft"], "An inaccurate first draft.");
            assert_eq!(review["source"], requests[0]["messages"][1]["content"]);
        }
    }

    #[tokio::test]
    async fn overlong_summaries_get_one_rewrite_without_truncation_or_model_fallback() {
        let (client, server) = mock_client(vec![
            ("stop", "draft".into()),
            ("stop", "x".repeat(141)),
            ("stop", "revised draft".into()),
            ("stop", "é".repeat(140)),
        ])
        .await;
        let report = project_to_tweet_with_client(&fixture(true), &client)
            .await
            .unwrap();
        assert_eq!(report.text, "é".repeat(140));
        assert_eq!(report.attempts.len(), 4);
        let requests = server.await.unwrap();
        assert!(requests[2]["messages"][3]["content"]
            .as_str()
            .unwrap()
            .contains("141 characters"));
        let (client, server) = mock_client(vec![
            ("stop", "draft".into()),
            ("stop", "x".repeat(141)),
            ("stop", "revised draft".into()),
            ("stop", "y".repeat(141)),
        ])
        .await;
        assert!(project_to_tweet_with_client(&fixture(false), &client)
            .await
            .unwrap_err()
            .to_string()
            .contains("after one rewrite"));
        assert_eq!(server.await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn incomplete_and_empty_responses_cannot_be_posted() {
        for (reason, text) in [
            ("length", "A valid-looking truncated summary."),
            ("stop", "   "),
        ] {
            let (client, server) = mock_client(vec![(reason, text.into())]).await;
            assert!(project_to_tweet_with_client(&fixture(true), &client)
                .await
                .is_err());
            assert_eq!(server.await.unwrap().len(), 1);
            let (client, server) =
                mock_client(vec![("stop", "Valid draft.".into()), (reason, text.into())]).await;
            assert!(project_to_tweet_with_client(&fixture(true), &client)
                .await
                .is_err());
            assert_eq!(server.await.unwrap().len(), 2);
        }
    }

    #[tokio::test]
    #[ignore = "Makes real official Z.ai requests through OpenRouter; requires OPEN_ROUTER_API_KEY"]
    async fn live_application_summary() {
        let projects: crate::models::Projects =
            serde_json::from_str(include_str!("../test_files/ExampleInput.json")).unwrap();
        let project = projects
            .data
            .iter()
            .find(|p| p.attributes.name.contains("development application"))
            .unwrap();
        let summary = project_to_tweet(project).await.unwrap();
        assert!(!summary.trim().is_empty());
        assert!(
            summary.chars().count() <= 140,
            "Summary exceeded the requested length: {summary}"
        );
        println!("{summary}");
    }

    #[test]
    fn test_html_to_markdown() {
        let description = r#"<p><img src="https://s3.ca-central-1.amazonaws.com/ehq-production-canada/17e7374a3b5c63231790827340fd28f639047b85/original/1675372309/aa7203d07fd579ed76f41da4a05ebf32_Capture.PNG?1675372309" style="width: 482px;" class="fr-fic fr-dib">Matthew Cheng Architect Inc. has applied to the City of Vancouver for permission to develop the following on this site:</p><ul><li>A new multiple dwelling building, containing six strata-titled dwelling units</li><li>A floor space ratio of 1.20 (approximately 6,650.24 sq. ft.)</li><li>A proposed height of approximately 33.3 ft.</li><li>Four parking spaces at the rear having access from the lane</li></ul><p>Under the site&rsquo;s existing <a href="https://bylaws.vancouver.ca/zoning/zoning-by-law-district-schedule-rm-8-all-districts.pdf">RM-8A zoning</a>, the application is &ldquo;conditional&rdquo; so it may be permitted. However, it requires the decision of the Director of Planning.</p>"#;

        let mut handlers = HashMap::<String, Box<dyn TagHandlerFactory>>::new();
        handlers.insert("img".to_string(), Box::new(IgnoreHandlerFactory));
        handlers.insert("a".to_string(), Box::new(TextOnlyHandlerFactory));

        let md = html2md::parse_html_custom(description, &handlers);

        let expected = "Matthew Cheng Architect Inc. has applied to the City of Vancouver for permission to develop the following on this site:

* A new multiple dwelling building, containing six strata-titled dwelling units
* A floor space ratio of 1.20 (approximately 6,650.24 sq. ft.)
* A proposed height of approximately 33.3 ft.
* Four parking spaces at the rear having access from the lane

Under the site’s existing RM-8A zoning, the application is “conditional” so it may be permitted. However, it requires the decision of the Director of Planning.";

        assert_eq!(md, expected);
    }
}
