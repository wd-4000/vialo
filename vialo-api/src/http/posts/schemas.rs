use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, Debug, PartialEq, Eq, Hash, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PostContentFormat {
    Markdown,
    Html,
    Plain,
}

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct PostFilterOptions {
    pub lang: Option<Vec<String>>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    pub current_only: Option<bool>,
    pub board_id: Option<Vec<i32>>,
    pub pinned_on: Option<bool>,
    #[serde(default)]
    pub content_format: HashSet<PostContentFormat>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreatePostSchema {
    pub board_id: i32,
    pub icon: Option<String>,
    pub title: Option<HashMap<String, String>>,
    pub content: Option<HashMap<String, String>>,
    pub location: Option<HashMap<String, String>>,
    pub pinned_until: Option<chrono::DateTime<chrono::Utc>>,
    pub event_from: Option<chrono::DateTime<chrono::Utc>>,
    pub event_to: Option<chrono::DateTime<chrono::Utc>>,
}
