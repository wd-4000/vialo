use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct QuickLinkModelTranslated {
    pub id: i32,
    pub label: Option<String>,
    pub link: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct JumboModelTranslated {
    pub id: i32,
    pub img: Option<String>,
    pub headline: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub link: Option<String>,
}
