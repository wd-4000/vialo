use crate::helpers::I18nMap;
use serde::{Deserialize, Serialize};
use sqlx::types::JsonValue;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "post_visibility", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PostVisibility {
    Public,
    LoggedIn,
    Board,
}

#[derive(Serialize)]
pub struct BoardPostModelRaw {
    pub id: i32,
    pub account_id: Uuid,
    pub board_id: i32,
    pub icon: Option<String>,
    pub title: Option<i32>,
    pub content: Option<i32>,
    pub location: Option<i32>,
    pub pinned_until: Option<chrono::DateTime<chrono::Utc>>,
    pub event_from: Option<chrono::DateTime<chrono::Utc>>,
    pub event_to: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct BoardPostModelTranslated {
    pub id: i32,
    pub account_id: Option<Uuid>,
    pub board_id: Option<i32>,
    pub icon: Option<String>,
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>, // markdown
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_plain: Option<String>,
    pub location: Option<String>,
    // pub pinned_until: Option<chrono::DateTime<chrono::Utc>>,
    pub event_from: Option<chrono::DateTime<chrono::Utc>>,
    pub event_to: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct BoardPostModelTranslatedWithPinnedOn {
    pub id: i32,
    pub account_id: Option<Uuid>,
    pub board_id: i32,
    pub icon: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub location: Option<String>,
    // pub pinned_until: Option<chrono::DateTime<chrono::Utc>>,
    pub event_from: Option<chrono::DateTime<chrono::Utc>>,
    pub event_to: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub pinned_on: Option<Vec<i32>>,
}

#[derive(Serialize, ToSchema)]
pub struct BoardPostModelTranslatedAllLanguages {
    pub id: i32,
    pub account_id: Option<Uuid>,
    pub board_id: Option<i32>,
    pub icon: Option<String>,
    #[schema(value_type = Option<I18nMap>)]
    pub title: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<I18nMap>)]
    pub content: Option<JsonValue>, // markdown
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<I18nMap>)]
    pub content_html: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<I18nMap>)]
    pub content_plain: Option<JsonValue>,
    #[schema(value_type = Option<I18nMap>)]
    pub location: Option<JsonValue>,
    // pub pinned_until: Option<chrono::DateTime<chrono::Utc>>,
    pub event_from: Option<chrono::DateTime<chrono::Utc>>,
    pub event_to: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct BoardPostIdModel {
    pub id: i32,
}

pub struct I18nStringModel {
    pub id: i32,
    pub lang: String,
    pub content: String,
}
