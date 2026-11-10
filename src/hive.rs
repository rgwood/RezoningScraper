//! Shape Your City moved from EngagementHQ to an OpenPoint "Hive" (Concrete CMS)
//! site on 2026-11-10. The old JSON API is gone. The new site has no public
//! project API, but its listing blocks load cards from a JSON endpoint, and
//! each project page has the full description, decision notices and documents.
//!
//! New-site projects get `hive-` prefixed IDs so they never collide with
//! EngagementHQ IDs in the legacy `Projects` table or the approval tables.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::models::{Attributes, Links1, Project, Relationships};

pub const BASE_URL: &str = "https://www.shapeyourcity.ca";
/// Pages whose listing blocks, together, cover every project. `/projects` omits
/// rezoning and development applications, which have their own pages.
pub const LISTING_PAGES: [&str; 3] = ["/projects", "/rezoning", "/development"];
pub const ID_PREFIX: &str = "hive-";
/// Cloudflare blocks some default library user agents.
pub const USER_AGENT: &str = concat!(
    "rezoning-scraper/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/rgwood/RezoningScraper)"
);
const MAX_LISTING_PAGES: u32 = 200;
const MAX_ATTEMPTS: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(1);
const DETAIL_CONCURRENCY: usize = 4;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ListingEntry {
    #[serde(rename = "projectID")]
    pub project_id: i64,
    pub project_name: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub project_description: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub project_status: Option<String>,
    pub project_path: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub project_image_src: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub project_date_num: Option<String>,
    #[serde(default)]
    pub project_category_array: Vec<String>,
    #[serde(default)]
    pub project_location_array: Vec<String>,
}

/// The site's PHP backend sends `false` (not null) for missing values.
fn lenient_string<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListingPage {
    result: Vec<ListingEntry>,
    #[serde(default)]
    more_to_load: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detail {
    /// Inner HTML of the main content blocks, joined.
    pub description: Option<String>,
    /// Highlighted notices, e.g. "The Director of Planning approved the application on ...".
    pub notices: Vec<String>,
}

pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(60))
        .build()?)
}

pub fn project_id(entry: &ListingEntry) -> String {
    format!("{ID_PREFIX}{}", entry.project_id)
}

/// Everything shown in a listing card. Detail pages are refetched when it changes.
pub fn fingerprint(entry: &ListingEntry) -> String {
    serde_json::to_string(entry).expect("listing entries always serialize")
}

/// The `load_more` routes of every project listing block on a page.
pub fn listing_routes(page: &str) -> Vec<String> {
    let html = Html::parse_document(page);
    let selector = Selector::parse("section.projects-list[data-route]").unwrap();
    let mut routes = Vec::new();
    for section in html.select(&selector) {
        let route = section.value().attr("data-route").unwrap().to_string();
        if !routes.contains(&route) {
            routes.push(route);
        }
    }
    routes
}

pub fn parse_listing(json: &str) -> Result<(Vec<ListingEntry>, bool)> {
    let page: ListingPage =
        serde_json::from_str(json).context("Unexpected project listing JSON")?;
    Ok((page.result, page.more_to_load))
}

pub fn parse_detail(page: &str) -> Detail {
    let html = Html::parse_document(page);
    let content = Selector::parse("main .hive-block-content").unwrap();
    let notice = Selector::parse("main .block-quote .hive-block-inner").unwrap();
    let parts: Vec<String> = html
        .select(&content)
        .map(|block| block.inner_html().trim().to_string())
        .filter(|html| !html.is_empty())
        .collect();
    let notices = html
        .select(&notice)
        .map(|block| {
            block
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|text| !text.is_empty())
        .collect();
    Detail {
        description: (!parts.is_empty()).then(|| parts.join("\n")),
        notices,
    }
}

/// Map a new-site project onto the model the rest of the scraper uses.
pub fn to_project(entry: &ListingEntry, detail: &Detail) -> Project {
    let state = match entry.project_status.as_deref() {
        Some("Closed") => "archived",
        _ => "published",
    };
    // The approval scanner reads decisions from `<p>` paragraphs, so give each
    // notice its own paragraph, matching EngagementHQ's archival messages.
    let archival_reason_message = (!detail.notices.is_empty()).then(|| {
        detail
            .notices
            .iter()
            .map(|notice| format!("<p>{}</p>", html_escape(notice)))
            .collect::<String>()
    });
    let mut tags = entry.project_category_array.clone();
    tags.extend(entry.project_location_array.iter().cloned());
    if let Some(status) = &entry.project_status {
        tags.push(status.clone());
    }
    let permalink = entry
        .project_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    Project {
        id: project_id(entry),
        project_type: "hive-project".into(),
        attributes: Attributes {
            name: entry.project_name.clone(),
            permalink,
            state: state.into(),
            visibility_mode: "public".into(),
            published_at: entry.project_date_num.clone(),
            description: detail.description.clone(),
            project_tag_list: tags,
            archival_reason_message,
            image_url: entry.project_image_src.clone(),
            meta_description: entry.project_description.clone(),
            access: true,
            source_fingerprint: Some(fingerprint(entry)),
            ..Default::default()
        },
        relationships: Relationships::default(),
        links: Links1 {
            self_link: entry.project_path.clone(),
        },
    }
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn get_text(client: &Client, url: &str) -> Result<String> {
    let mut attempt = 1;
    loop {
        let result = async {
            client
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await
        }
        .await;
        match result {
            Ok(text) => return Ok(text),
            Err(error)
                if attempt < MAX_ATTEMPTS
                    && (error.is_timeout()
                        || error.is_connect()
                        || error.is_request()
                        || error.is_body()
                        || error.status().is_some_and(|s| {
                            s.is_server_error() || s == reqwest::StatusCode::TOO_MANY_REQUESTS
                        })) =>
            {
                let delay = RETRY_DELAY * 2_u32.pow(attempt - 1);
                eprintln!(
                    "Request attempt {attempt}/{MAX_ATTEMPTS} failed for {url}: {error}; retrying in {}s",
                    delay.as_secs()
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("request failed after {attempt} attempt(s): {url}"))
            }
        }
    }
}

/// Every project card from every listing block, deduplicated by project ID.
pub async fn fetch_listings(client: &Client, base_url: &str) -> Result<Vec<ListingEntry>> {
    let mut routes = Vec::new();
    for path in LISTING_PAGES {
        let page = get_text(client, &format!("{base_url}{path}")).await?;
        let found = listing_routes(&page);
        if found.is_empty() {
            bail!(
                "No project listing blocks found on {base_url}{path}; did the site change again?"
            );
        }
        for route in found {
            if !routes.contains(&route) {
                routes.push(route);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    // The server takes ~4s per listing page and only returns 10 cards per page,
    // so fetch a few pages of each listing at once. Pages past the end wrap
    // around rather than coming back empty, so stop at the first page that
    // says there are no more and ignore the rest of that window.
    for route in routes {
        let mut first_page = 1;
        'pages: loop {
            let mut tasks = tokio::task::JoinSet::new();
            for page_number in first_page..first_page + DETAIL_CONCURRENCY as u32 {
                let client = client.clone();
                let separator = if route.contains('?') { '&' } else { '?' };
                let url = format!("{route}{separator}page={page_number}");
                tasks.spawn(async move {
                    let result = match get_text(&client, &url).await {
                        Ok(text) => parse_listing(&text)
                            .with_context(|| format!("Bad listing response from {url}")),
                        Err(error) => Err(error),
                    };
                    (page_number, result)
                });
            }
            let mut window = tasks.join_all().await;
            window.sort_by_key(|(page_number, _)| *page_number);
            for (page_number, result) in window {
                let (page, more) = result?;
                for entry in page {
                    if seen.insert(entry.project_id) {
                        entries.push(entry);
                    }
                }
                if !more {
                    break 'pages;
                }
                if page_number >= MAX_LISTING_PAGES {
                    bail!("Listing {route} has more than {MAX_LISTING_PAGES} pages");
                }
            }
            first_page += DETAIL_CONCURRENCY as u32;
        }
    }
    if entries.is_empty() {
        bail!("Shape Your City listings returned no projects");
    }
    Ok(entries)
}

pub async fn fetch_detail(client: &Client, entry: &ListingEntry) -> Result<Detail> {
    let page = get_text(client, &entry.project_path).await?;
    Ok(parse_detail(&page))
}

pub struct Fetched {
    pub projects: Vec<Project>,
    /// New projects whose detail page failed; they are retried next run.
    pub failures: Vec<String>,
    pub detail_fetches: usize,
}

/// Build current projects, reusing stored details when a listing card is unchanged.
pub async fn fetch_projects(
    client: &Client,
    base_url: &str,
    stored: &HashMap<String, Project>,
) -> Result<Fetched> {
    let entries = fetch_listings(client, base_url).await?;
    let mut to_fetch = Vec::new();
    let mut projects: Vec<Option<Project>> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        match stored.get(&project_id(entry)) {
            Some(old)
                if old.attributes.source_fingerprint.as_deref() == Some(&fingerprint(entry)) =>
            {
                projects.push(Some(old.clone()))
            }
            _ => {
                projects.push(None);
                to_fetch.push(index);
            }
        }
    }
    let detail_fetches = to_fetch.len();
    let mut failures = Vec::new();
    for chunk in to_fetch.chunks(DETAIL_CONCURRENCY) {
        let mut tasks = tokio::task::JoinSet::new();
        for &index in chunk {
            let client = client.clone();
            let entry = entries[index].clone();
            tasks.spawn(async move { (index, fetch_detail(&client, &entry).await) });
        }
        while let Some(joined) = tasks.join_next().await {
            let (index, result) = joined?;
            let entry = &entries[index];
            match result {
                Ok(detail) => projects[index] = Some(to_project(entry, &detail)),
                Err(error) => {
                    eprintln!("Could not fetch {}: {error:#}", entry.project_path);
                    failures.push(format!("{}: {error:#}", entry.project_path));
                    // Keep the previous snapshot; its old fingerprint forces a retry next run.
                    projects[index] = stored.get(&project_id(entry)).cloned();
                }
            }
        }
    }
    Ok(Fetched {
        projects: projects.into_iter().flatten().collect(),
        failures,
        detail_fetches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"{"result":[{"projectID":2008,"projectName":"6525-6577 Oak St rezoning application","projectDescription":"Approved on July 27, 2026","projectStatus":"Closed","projectPath":"https://www.shapeyourcity.ca/6525-6575-oak","projectImageName":"Render.jpg","projectImageSrc":"https://example.com/Render.jpg","projectImageFileID":1819,"imgOverlay":false,"projectDateNum":"2026-10-20","projectDateStr":"Oct 20, 2026","newsDateNum":null,"newsDateStr":null,"projectLocation":"","projectLocationArray":[],"projectCategory":"Rezoning","projectCategoryArray":["Rezoning"],"projectCategories":[{"class":"rezoning","value":"Rezoning"}],"projectColour":"project-colour-standard"}],"moreToLoad":true}"#;

    #[test]
    fn finds_listing_routes() {
        let page = r#"<section id="hive-block-1" class="hive-block projects-list" data-bID="1" data-route="https://www.shapeyourcity.ca/ccm/the_hive_projects/tools/the_hive_projects_list/load_more/1" data-page="1"></section>
            <section class="hive-block projects-list" data-route="https://www.shapeyourcity.ca/ccm/the_hive_projects/tools/the_hive_projects_list/load_more/2"></section>
            <div data-route="https://example.com/not-a-listing"></div>"#;
        assert_eq!(
            listing_routes(page),
            vec![
                "https://www.shapeyourcity.ca/ccm/the_hive_projects/tools/the_hive_projects_list/load_more/1",
                "https://www.shapeyourcity.ca/ccm/the_hive_projects/tools/the_hive_projects_list/load_more/2",
            ]
        );
    }

    #[test]
    fn parses_listing_json() {
        let (entries, more) = parse_listing(LISTING).unwrap();
        assert!(more);
        assert_eq!(entries[0].project_id, 2008);
        assert_eq!(entries[0].project_category_array, vec!["Rezoning"]);
    }

    #[test]
    fn maps_development_approval_into_decisions() {
        let page = r#"<html><body><main class="main"><div class="row"><div class="hive-block block-quote"><aside><div class="hive-block-inner ljs">
            The Director of Planning approved the application on October 14, 2026, subject to conditions. A Development Permit may be issued once all conditions have been satisfied.
            </div></aside></div></div>
            <div class="hive-block hive-block-content ljs"><p>An application to change the use.</p></div></main></body></html>"#;
        let detail = parse_detail(page);
        assert_eq!(
            detail.description.as_deref(),
            Some("<p>An application to change the use.</p>")
        );
        let entry = ListingEntry {
            project_id: 1,
            project_name: "328 W 10th Ave (DP-2026-00313) dev appln".into(),
            project_description: Some("Approved on October 14, 2026".into()),
            project_status: Some("Closed".into()),
            project_path: "https://www.shapeyourcity.ca/328-w-10-ave".into(),
            project_image_src: None,
            project_date_num: None,
            project_category_array: vec!["Development".into()],
            project_location_array: vec![],
        };
        let project = to_project(&entry, &detail);
        assert_eq!(project.id, "hive-1");
        assert_eq!(project.attributes.permalink, "328-w-10-ave");
        assert_eq!(project.attributes.state, "archived");
        let found = crate::approvals::decisions(&project);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "development_approved_with_conditions");
        assert_eq!(found[0].date.as_deref(), Some("2026-10-14"));
    }

    #[test]
    fn maps_rezoning_approval_from_content() {
        let page = r#"<main><div class="hive-block hive-block-content ljs"><p><strong>This application was approved by Council following the Public Hearings on July 23 and 27, 2026.</strong></p><p>Details.</p></div></main>"#;
        let (entries, _) = parse_listing(LISTING).unwrap();
        let project = to_project(&entries[0], &parse_detail(page));
        let found = crate::approvals::decisions(&project);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "rezoning_approved");
    }

    #[test]
    fn tolerates_php_false_for_missing_values() {
        let json = r#"{"result":[{"projectID":5,"projectName":"No image","projectDescription":"","projectStatus":"Active","projectPath":"https://www.shapeyourcity.ca/x","projectImageSrc":false,"projectImageFileID":false,"projectDateNum":"2026-11-01","newsDateNum":null,"projectCategoryArray":[],"projectLocationArray":[]}],"moreToLoad":false}"#;
        let (entries, more) = parse_listing(json).unwrap();
        assert!(!more);
        assert_eq!(entries[0].project_image_src, None);
        assert_eq!(entries[0].project_description, None);
    }

    #[test]
    fn fingerprint_changes_with_listing() {
        let (mut entries, _) = parse_listing(LISTING).unwrap();
        let before = fingerprint(&entries[0]);
        entries[0].project_description = Some("Enacted".into());
        assert_ne!(before, fingerprint(&entries[0]));
    }
}
