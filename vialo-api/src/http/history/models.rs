use serde::{Deserialize, Serialize};
use sqlx::{prelude::FromRow, types::JsonValue};
use utoipa::ToSchema;
use uuid::Uuid;

/*
id int PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
table_name TEXT,
record_id int,  -- NB/TODO: Schedule PII-related logs for deletion when a user gets deleted!
operation_type tgop,
created_at TIMESTAMPTZ DEFAULT now(),
delete_on TIMESTAMPTZ,
caused_by_account uuid REFERENCES accounts_people (id) ON DELETE SET NULL,
caused_by_subsystem subsystem_type,
diff jsonb
*/
#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "subsystem_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Subsystem {
    App,
    Bookable,
    Printer,
    Ppsk,
    Email,
    Sysop,
}

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "tgop", rename_all = "UPPERCASE")]
#[serde(rename_all = "lowercase")]
pub enum TgOp {
    Insert,
    Update,
    Delete,
}

#[derive(Serialize, FromRow)]
pub struct SchemaModel {
    pub res: JsonValue,
}

#[derive(Serialize, FromRow, ToSchema)]
pub struct HistoryEntryModel {
    pub id: i32,
    pub table_name: Option<String>,
    pub object_name: Option<String>,

    pub record_id: Option<i32>,
    pub record_uuid: Option<Uuid>,
    pub operation_type: Option<TgOp>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub delete_on: Option<chrono::DateTime<chrono::Utc>>,
    pub caused_by_account: Option<JsonValue>,
    pub caused_by_subsystem: Option<Subsystem>,
    pub diff: Option<JsonValue>,
}
