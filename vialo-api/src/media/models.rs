use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crate::http::media::models::{
    MediaAttachment, MediaContent, MediaContext, MediaModel, MediaStatus, MediaType,
};

/// Flat DB row; converted into the tagged model
pub struct MediaRow {
    pub id: Uuid,
    pub media_type: MediaType,
    pub context: MediaContext,
    pub mime: String,
    pub filename: Option<String>,
    pub size_bytes: i64,
    pub status: MediaStatus,
    pub post_id: Option<i32>,
    pub position: i32,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub variants: Option<Vec<i32>>,
}

impl From<MediaRow> for MediaModel {
    fn from(r: MediaRow) -> Self {
        Self {
            id: r.id,
            mime: r.mime,
            size_bytes: r.size_bytes,
            status: r.status,
            content: match r.media_type {
                MediaType::Image => MediaContent::Image {
                    width: r.width,
                    height: r.height,
                    variants: r.variants.unwrap_or_default(),
                },
                MediaType::Document => MediaContent::Document { filename: r.filename.unwrap_or_default() },
            },
            attachment: match r.context {
                MediaContext::Post => MediaAttachment::Post { post_id: r.post_id, position: r.position },
                MediaContext::Avatar => MediaAttachment::Avatar,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct JobData {
    pub media_id: Uuid,
}
