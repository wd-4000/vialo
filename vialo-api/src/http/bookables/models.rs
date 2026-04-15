use crate::helpers::PgDateTime;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::types::JsonValue;

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize)]
#[sqlx(type_name = "bookable_status_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum BookableStatus {
    Available,
    QuickUnlock,
    Waiting,
    Active,
    Maintenance,
}

#[derive(Serialize, Clone)]
pub struct BookableAssetStatus {
    pub id: i32,
    pub asset_type: i32,
    pub status: BookableStatus,
    pub begins: Option<NaiveDateTime>,
    pub ends: Option<PgDateTime>,
}

#[derive(Serialize)]
pub struct BookableAssetTranslatedWithStatus {
    pub id: Option<i32>,
    pub icon: Option<String>,
    pub name: Option<String>,
    pub asset_type: Option<i32>,
    pub status: BookableStatus,
    pub begins: Option<NaiveDateTime>,
    pub ends: Option<PgDateTime>,
}

#[derive(Serialize)]
pub struct BookableAssetTranslated {
    pub id: i32,
    pub icon: Option<String>,
    pub name: Option<String>,
    pub asset_type: Option<i32>,
    pub status: BookableStatus,
}

#[derive(Serialize)]
pub struct BookableAssetTranslatedAllLanguages {
    pub id: i32,
    pub icon: Option<String>,
    pub name: Option<JsonValue>,
    pub asset_type: Option<i32>,
    pub slug: Option<String>,
    pub connector: Option<i32>,
    pub connector_output_id: Option<i32>,
}

#[derive(Serialize)]
pub struct BoardPostIdModel {
    pub id: i32,
}

pub struct I18nStringModel {
    pub id: i32,
    pub lang: String,
    pub content: String,
}
