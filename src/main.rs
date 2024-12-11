use anyhow::{anyhow, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use scraper::{Html, Selector};
use serde_json::Value;
use std::time::Duration;

mod db;
mod models;
use db::{Database, Token};
use models::{Project, Projects};


fn main() -> Result<()> {
    println!("Welcome to RezoningScraper");

    // Open database
    println!("Opening database...");
    let mut db = Database::new_from_file("rezoning_scraper.db")?;

    // Get token
    println!("Loading token...");
    let token = get_token_from_db_or_website(&mut db)?;

    // Setup HTTP client
    println!("Querying API...");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;

    // Fetch projects
    let start = std::time::Instant::now();
    let latest_projects = fetch_all_projects(&client, &token.jwt)?;
    println!(
        "API query finished: retrieved {} projects in {}ms",
        latest_projects.len(),
        start.elapsed().as_millis()
    );

    // Compare against database
    println!("Comparing against projects in local database...");
    let start = std::time::Instant::now();

    let mut new_projects = Vec::new();
    let mut changed_projects = Vec::new();

    for project in &latest_projects {
        if db.contains_project(&project.id)? {
            let old_version = db.get_project(&project.id)?;

            // For now just compare name field as an example
            if old_version.attributes.name != project.attributes.name {
                let change = ProjectChange {
                    field: "name".to_string(),
                    old_value: old_version.attributes.name,
                    new_value: project.attributes.name.clone(),
                };
                changed_projects.push((project.clone(), vec![change]));
            }
        } else {
            new_projects.push(project.clone());
        }

        db.upsert_project(project)?;
    }

    println!(
        "Upserted {} projects to DB in {}ms",
        latest_projects.len(),
        start.elapsed().as_millis()
    );
    println!(
        "Found {} new projects and {} modified projects",
        new_projects.len(),
        changed_projects.len()
    );

    // Print results
    if !new_projects.is_empty() {
        println!("\nNew Projects:");
        for project in new_projects {
            println!("\n{}", project.attributes.name);
            println!("State: {}", project.attributes.state);
            if !project.attributes.project_tag_list.is_empty() {
                println!("Tags: {}", project.attributes.project_tag_list.join(", "));
            }
            println!("URL: {}", project.links.self_link);
        }
    }

    if !changed_projects.is_empty() {
        println!("\nChanged Projects:");
        for (project, changes) in changed_projects {
            println!("\n{}", project.attributes.name);
            for change in changes {
                println!(
                    "{}: '{}' -> '{}'",
                    change.field, change.old_value, change.new_value
                );
            }
        }
    }

    Ok(())
}

fn get_token_from_db_or_website(db: &mut Database) -> Result<Token> {
    // Check if we have a valid token in the DB
    if let Some(token) = db.get_token()? {
        let now = Utc::now();
        if token.expiration > now + chrono::Duration::minutes(1) {
            println!(
                "Loaded API token from database. Cached token will expire on {}",
                token.expiration
            );
            return Ok(token);
        }
    }

    println!("Getting latest anonymous user token from shapeyourcity.ca");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;

    let html = client
        .get("https://shapeyourcity.ca/embeds/projectfinder")
        .send()?
        .text()?;

    let jwt = extract_token_from_html(&html)?;
    let expiration = get_expiration_from_encoded_jwt(&jwt)?;

    println!("Retrieved JWT with expiration date {}", expiration);

    let token = Token { expiration, jwt };

    db.set_token(&token)?;
    println!("Cached JWT in local database");

    Ok(token)
}

fn fetch_all_projects(client: &reqwest::blocking::Client, jwt: &str) -> Result<Vec<Project>> {
    const RESULTS_PER_PAGE: u32 = 200;
    let mut all_projects = Vec::new();
    let mut next_url = Some(format!(
        "https://shapeyourcity.ca/api/v2/projects?per_page={}",
        RESULTS_PER_PAGE
    ));
    let mut page_count = 0;

    while let Some(url) = next_url {
        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", jwt))
            .send()?
            .json::<Projects>()?;

        page_count += 1;
        println!(
            "Retrieved page {} ({} items)",
            page_count,
            response.data.len()
        );

        all_projects.extend(response.data);
        next_url = response.links.next;
    }

    Ok(all_projects)
}

#[derive(Debug)]
struct ProjectChange {
    field: String,
    old_value: String,
    new_value: String,
}

fn extract_token_from_html(html: &str) -> Result<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("script#__NEXT_DATA__").unwrap();

    let script = document
        .select(&selector)
        .next()
        .ok_or_else(|| anyhow!("Could not find NEXT_DATA tag"))?;

    let json: Value = serde_json::from_str(&script.inner_html())
        .map_err(|e| anyhow!("Could not parse NEXT_DATA JSON: {}", e))?;

    // Navigate the JSON structure to get the token
    json["props"]["pageProps"]["initialState"]["anonymousUser"]["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Could not find token in JSON"))
        .map(String::from)
}

fn get_expiration_from_encoded_jwt(jwt: &str) -> Result<DateTime<Utc>> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return Err(anyhow!("Invalid JWT format"));
    }

    // Decode the payload (middle part)
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| anyhow!("Failed to decode JWT payload: {}", e))?;

    let json: Value =
        serde_json::from_slice(&payload).map_err(|e| anyhow!("Failed to parse JWT JSON: {}", e))?;

    // Extract exp claim
    let exp = json["exp"]
        .as_i64()
        .ok_or_else(|| anyhow!("No expiration in JWT"))?;

    Ok(Utc
        .timestamp_opt(exp, 0)
        .single()
        .ok_or_else(|| anyhow!("Invalid timestamp"))?)
}

// tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_deserialize() {
        let json = include_str!("../test_files/ExampleInput.json");
        let result = serde_json::from_str::<Projects>(json).expect("Should deserialize");

        assert_eq!(result.data.len(), 30);
    }

    /* { "data": {
           "user_id": 467419949,
           "user_type": "AnonymousUser"
         },
         "exp": 1638294321,
         "iat": 1638121521,
         "iss": "Bang The Table Pvt Ltd",
         "jti": "49b984aa47751d338fc3baf7897e9685"
    }
    */
    const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJpYXQiOjE2MzgxMjE1MjEsImp0aSI6IjQ5Yjk4NGFhNDc3NTFkMzM4ZmMzYmFmNzg5N2U5Njg1IiwiZXhwIjoxNjM4Mjk0MzIxLCJpc3MiOiJCYW5nIFRoZSBUYWJsZSBQdnQgTHRkIiwiZGF0YSI6eyJ1c2VyX2lkIjo0Njc0MTk5NDksInVzZXJfdHlwZSI6IkFub255bW91c1VzZXIifX0.p7FGWkT_7sWtC4gAsQ_HgaX-Z8aw88QQiGdHITmQleQ";
    const EXPIRATION_IN_UNIX_SECONDS: i64 = 1638294321;

    #[test]
    fn test_extract_jwt() {
        let html = include_str!("../test_files/projectFinder.html");
        let jwt = extract_token_from_html(html).expect("Should extract JWT");
        assert_eq!(jwt, JWT);
    }

    #[test]
    fn test_extract_expiration_from_jwt() {
        let expiration = get_expiration_from_encoded_jwt(JWT).expect("Should parse expiration");
        assert_eq!(expiration.timestamp(), EXPIRATION_IN_UNIX_SECONDS);
    }
}
