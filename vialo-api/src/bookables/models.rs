use crate::{helpers::PgDateTime, http::bookables::models::BookableStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Serialize, Debug)]
pub struct BookableAssetStatusWithConnector {
    pub id: i32,
    pub status: BookableStatus,
    pub begins: Option<DateTime<Utc>>,
    pub ends: Option<PgDateTime>,
    pub connector: i32,
    pub connector_output_id: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExpiredAppointmentMessage {
    pub id: uuid::Uuid,
    pub account_id: uuid::Uuid,
    pub full_name: Option<String>,
    pub email: String,
    pub asset_name: String,
    pub credits_refunded: i32,
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
