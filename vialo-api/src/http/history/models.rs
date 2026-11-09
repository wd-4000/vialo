use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::types::JsonValue;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::http::util::models::AccountPersonEmbed;

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "subsystem_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Subsystem {
    /// Generic value for "done by the app"
    App,
    /// Handles bookable activation, appointment expiry, connector sync
    Bookable,
    /// Creates/deletes printer users, sets print limits, commits credit deductions
    Printer,
    /// Pushes PPSK Wi‑Fi credentials to a network management system and syncs VLANs
    Nms,
    /// Sends notification emails
    Email,
    /// Sign-in for people directly changing the database
    Sysop,
    /// DHCP
    DHCP,
    /// Imported
    Import,
    /// Media encoding
    Media,
}

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "tgop", rename_all = "UPPERCASE")]
#[serde(rename_all = "lowercase")]
pub enum TgOp {
    Insert,
    Update,
    Delete,
}

#[derive(Serialize)]
pub struct SchemaModel {
    pub res: JsonValue,
}

#[derive(Serialize, ToSchema)]
pub struct DiffUpdateEntry {
    pub from: Option<JsonValue>,
    pub to: Option<JsonValue>,
}

#[derive(ToSchema)]
pub struct HistoryEntryDiff(HashMap<String, DiffUpdateEntry>);

#[derive(Serialize, ToSchema)]
pub struct HistoryEntryModel {
    pub id: i32,
    pub table_name: Option<String>,
    pub object_name: Option<String>,

    pub record_id: Option<i32>,
    pub record_uuid: Option<Uuid>,
    pub operation_type: Option<TgOp>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub delete_on: Option<chrono::DateTime<chrono::Utc>>,
    pub caused_by_account: Option<AccountPersonEmbed>,
    pub caused_by_subsystem: Option<Subsystem>,
    #[schema(value_type = HistoryEntryDiff)]
    pub diff: Option<JsonValue>,
}
