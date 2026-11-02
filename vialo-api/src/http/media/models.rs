use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "media_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Image,
    Document,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "media_context", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MediaContext {
    Post,
    Avatar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "media_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MediaStatus {
    Pending,
    Ready,
    Error,
}

/// What the file is
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum MediaContent {
    Image {
        width: Option<i32>,
        height: Option<i32>,
        /// Encoded widths. Request any size and the nearest larger one is served
        variants: Vec<i32>,
    },
    Document {
        filename: String,
    },
}

/// Where the file lives
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "context")]
pub enum MediaAttachment {
    /// `post_id` None = staged
    Post {
        post_id: Option<i32>,
        position: i32,
    },
    Avatar,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct MediaModel {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub mime: String,
    pub size_bytes: i64,
    pub status: MediaStatus,
    pub content: MediaContent,
    pub attachment: MediaAttachment,
}
