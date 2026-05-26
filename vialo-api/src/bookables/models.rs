use crate::{helpers::PgDateTime, http::bookables::models::BookableStatus};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Serialize, Debug)]
pub struct BookableAssetStatusWithConnector {
    pub id: i32,
    pub status: BookableStatus,
    pub begins: Option<NaiveDateTime>,
    pub ends: Option<PgDateTime>,
    pub connector: i32,
    pub connector_output_id: i32,
}

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Serialize, Deserialize, ToSchema)]
#[sqlx(
    type_name = "bookable_appointment_cancellation_reason",
    rename_all = "lowercase"
)]
#[serde(rename_all = "lowercase")]
pub enum CancellationReason {
    Expired,
    User,
    Admin,
}
