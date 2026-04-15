use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Default)]
pub struct PostFilterOptions {
    pub lang: Option<Vec<String>>,
    pub page: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug)]
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
