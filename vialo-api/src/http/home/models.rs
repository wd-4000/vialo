use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct QuickLinkModelTranslated {
    pub id: Option<i32>,
    pub label: Option<String>,
    pub link: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct JumboModelTranslated {
    pub id: Option<i32>,
    pub img: Option<String>,
    pub headline: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub link: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct BoardPostModelHomeTranslated {
    pub id: Option<i32>,
    pub icon: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub location: Option<String>,
    pub pinned_until: Option<chrono::DateTime<chrono::Utc>>,
    pub event_from: Option<chrono::DateTime<chrono::Utc>>,
    pub event_to: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}
