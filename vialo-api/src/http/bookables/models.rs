use crate::helpers::{I18nMap, PgDateTime};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::JsonValue;
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[schema(rename_all = "snake_case")]
#[sqlx(type_name = "bookable_status_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum BookableStatus {
    Available,
    QuickUnlock,
    Waiting,
    Active,
    Maintenance,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct BookableAssetStatus {
    pub id: i32,
    pub asset_type_id: i32,
    pub status: BookableStatus,
    pub begins: Option<DateTime<Utc>>,
    pub ends: Option<PgDateTime>,
}

#[derive(Serialize, ToSchema)]
pub struct BookableAssetTranslatedWithStatus {
    pub id: i32,
    pub icon: Option<String>,
    pub name: Option<String>,
    pub asset_type_id: i32,
    pub status: BookableStatus,
    #[schema(format = DateTime)]
    pub begins: Option<DateTime<Utc>>,
    pub ends: Option<PgDateTime>,
}

#[derive(Serialize, ToSchema)]
pub struct BookableAssetTranslated {
    pub id: i32,
    pub icon: Option<String>,
    pub name: Option<String>,
    pub asset_type_id: i32,
    pub status: BookableStatus,
}

#[derive(Serialize, ToSchema)]
pub struct BookableAssetTranslatedAllLanguages {
    pub id: i32,
    pub icon: Option<String>,
    #[schema(value_type = Option<I18nMap>)]
    pub name: Option<JsonValue>,
    pub asset_type_id: i32,
    pub slug: Option<String>,
    pub connector: Option<i32>,
    pub connector_output_id: Option<i32>,
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
