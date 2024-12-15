use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SlackMessage {
    json: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Projects {
    pub data: Vec<Project>,
    pub links: Links,
    pub meta: Meta,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Links {
    #[serde(rename = "self")]
    pub self_link: String,
    pub first: String,
    pub prev: Option<serde_json::Value>,
    pub next: Option<String>,
    pub last: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Meta {
    pub all: i32,
    pub published: i32,
    pub draft: i32,
    pub archived: i32,
    pub hidden: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    #[serde(rename = "type")]
    pub project_type: String,
    pub attributes: Attributes,
    pub relationships: Relationships,
    pub links: Links1,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Attributes {
    pub name: String,
    pub permalink: String,
    pub state: String,
    pub visibility_mode: String,
    pub published_at: Option<String>,
    pub banner_url: Option<String>,
    pub description: Option<String>,
    pub project_tag_list: Vec<String>,
    pub created_at: String,
    pub archival_reason_message: Option<String>,
    pub image_url: Option<String>,
    pub image_caption: Option<String>,
    pub image_description: Option<String>,
    pub meta_description: Option<String>,
    pub parent_id: Option<i32>,
    pub access: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Relationships {
    pub site: Site,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Site {
    pub data: Data,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Data {
    pub id: String,
    #[serde(rename = "type")]
    pub data_type: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Links1 {
    #[serde(rename = "self")]
    pub self_link: String,
}
