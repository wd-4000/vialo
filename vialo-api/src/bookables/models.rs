use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use strum::EnumDiscriminants;
use utoipa::ToSchema;

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
    Maintenance,
}

#[derive(Clone, Debug, PartialEq, sqlx::Type, Serialize, Deserialize, ToSchema)]
#[sqlx(type_name = "bookable_maintenance_refund", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceRefund {
    Refund,
    NoRefund,
}

#[derive(Clone, Debug, PartialEq, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "bookable_connector_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum BookableConnectorType {
    Netio,
    Miele,
}
