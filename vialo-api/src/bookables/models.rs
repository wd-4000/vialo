use crate::{helpers::PgDateTime, http::bookables::models::BookableStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use strum::EnumDiscriminants;
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

/// What happened to an appointment, this specifc type is for notification management
#[derive(Clone, Debug, Serialize, EnumDiscriminants)]
#[strum_discriminants(
    name(BookableEventKind),
    derive(Serialize, Deserialize, ToSchema, sqlx::Type),
    serde(rename_all = "snake_case"),
    sqlx(type_name = "bookable_event_kind", rename_all = "snake_case")
)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum BookableEvent {
    Booked {
        credits: i32,
    },
    Upcoming,
    Canceled {
        reason: Option<CancellationReason>,
        credits_refunded: i32,
    },
    Expired {
        credits_refunded: i32,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct BookableEventMessage {
    pub appointment_id: uuid::Uuid,
    pub account_id: uuid::Uuid,
    pub asset_name: String,
    pub begins: DateTime<Utc>,
    pub ends: DateTime<Utc>,
    pub event: BookableEvent,
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
