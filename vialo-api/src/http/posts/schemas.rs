use crate::helpers::I18nMap;

use super::models::PostVisibility;
use serde::{Deserialize, Serialize};
use serde_with::{DefaultOnNull, serde_as};
use std::collections::HashSet;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

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

#[serde_as]
#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreatePostSchema {
    pub board_id: i32,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_64", default)]
    pub icon: Option<String>,
    pub title: Option<I18nMap>,
    pub content: Option<I18nMap>,
    pub location: Option<I18nMap>,
    pub pinned_until: Option<chrono::DateTime<chrono::Utc>>,
    pub event_from: Option<chrono::DateTime<chrono::Utc>>,
    pub event_to: Option<chrono::DateTime<chrono::Utc>>,
    pub visibility: PostVisibility,
    #[serde(default)]
    #[serde_as(deserialize_as = "DefaultOnNull")]
    pub media: Vec<Uuid>,
    #[serde(default)]
    pub cover_id: Option<Uuid>,
}
