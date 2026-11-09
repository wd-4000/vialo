use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::helpers::I18nMap;

#[derive(Deserialize, Debug, Default, ToSchema, IntoParams)]
pub struct PostFilterOptions {
    pub lang: Option<Vec<String>>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreatePostSchema {
    pub board_id: i32,
    pub icon: Option<String>,
    pub title: Option<I18nMap>,
    pub content: Option<I18nMap>,
    pub location: Option<I18nMap>,
    pub pinned_until: Option<chrono::DateTime<chrono::Utc>>,
    pub event_from: Option<chrono::DateTime<chrono::Utc>>,
    pub event_to: Option<chrono::DateTime<chrono::Utc>>,
}
