use super::permissions::{BookablePerm, require_asset_type_perm};
use crate::bookables::CancellationReason;
use crate::helpers::{PgDateTime, encryption, grab_authd_conn_subsystem};
use crate::http::history::models::Subsystem;
use crate::http::util::models::{AccountEmbed, IdOrAllQuery, IdOrMeOrAllQuery};
use crate::http::util::{
    JsonE, User, VialoError, clamp_pagination, grab_authd_conn_user, grab_trans,
};
use crate::permissions::{AppRole, check_app_role};
use crate::{AppState, health};
use axum::extract::{ConnectInfo, Path};
use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::{TypedHeader, extract::Query};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{query, query_scalar};
use sqlx_conditional_queries::conditional_query_as;
use std::i64;
use std::net::SocketAddr;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct BookableAppointmentFilterOptions {
    pub lang: Option<Vec<String>>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub account_id: IdOrMeOrAllQuery,
    pub to: Option<DateTime<Utc>>,
    pub search: Option<String>,
    #[serde(default)]
    pub include_cancelled: bool,
}

#[derive(Serialize, ToSchema)]
pub struct BookableAppointmentType {
    pub id: Uuid,
    pub asset_id: i32,
    pub transaction_id: Option<Uuid>,
    pub account: Option<AccountEmbed>,
    pub begins: Option<DateTime<Utc>>,
    pub ends: PgDateTime,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub cancellation_reason: Option<CancellationReason>,
    #[schema(format = DateTime)]
    pub activated: Option<chrono::DateTime<chrono::Utc>>,
    pub maintenance: Option<bool>,
}

#[derive(Deserialize, Debug, Default, ToSchema)]
pub struct KioskActivateBody {
    #[serde(default)]
    pub pin: Option<String>,
}

#[utoipa::path(post, path = "/bookables/appointments/{id}/activate", request_body=KioskActivateBody, responses((status = 204, description = "Activated")))]
pub async fn activate(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user_o): Extension<Option<User>>,
    authorization: Option<TypedHeader<headers::Authorization<headers::authorization::Bearer>>>,
    headers: HeaderMap,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    body: Option<Json<KioskActivateBody>>,
) -> Result<impl IntoResponse, VialoError> {
    // A valid kiosk credential: activate on behalf of the appointment's owner
    let kiosk = authorization.and_then(|TypedHeader(h)| {
        let token = h.0.token();
        data.config
            .bookables
            .as_ref()?
            .kiosk
            .as_ref()
            .filter(|k| k.matches(token))
            .cloned()
    });

    if let Some(kiosk) = kiosk {
        let Some(Json(body)) = body else {
            return Err(VialoError::Forbidden());
        };
        let Some(pin) = body.pin else {
            return Err(VialoError::Forbidden());
        };

        // The PIN is brute-forceable, so e throttle it per IP.
        data.rate_limiters
            .check_kiosk_pin(&headers, connect_info.as_deref())?;

        return kiosk_activate(&data, id, &kiosk, &pin).await;
    }

    let Some(user) = user_o else {
        return Err(VialoError::Forbidden());
    };

    // Implicit permission check in the query
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let res = query!(
        "UPDATE bookable_appointments SET activated = NOW() WHERE id = $1 AND account_id = $2",
        id,
        user.id
    )
    .execute(&mut *conn)
    .await?;

    if res.rows_affected() != 1 {
        return Err(VialoError::NotFound());
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn kiosk_activate(
    data: &Arc<AppState>,
    id: Uuid,
    kiosk: &crate::config::KioskConfig,
    pin: &str,
) -> Result<StatusCode, VialoError> {
    let record = query!(
        r#"SELECT ba.account_id, ba.activated, b.asset_type_id
           FROM bookable_appointments ba
           JOIN bookable_assets b ON b.id = ba.asset_id
           WHERE ba.id = $1"#,
        id,
    )
    .fetch_optional(&data.db)
    .await?
    .ok_or(VialoError::NotFound())?;

    // The token is scoped to asset types, same as the queue read.
    if !kiosk.allows(record.asset_type_id) {
        return Err(VialoError::Forbidden());
    }

    if record.activated.is_some() {
        return Err(VialoError::AppError(
            StatusCode::FORBIDDEN,
            "already_activated".into(),
        ));
    }

    let stored = query_scalar!(
        "SELECT amenities_pin FROM accounts_people WHERE id = $1",
        record.account_id
    )
    .fetch_optional(&data.db)
    .await?
    .flatten();

    // No credentials and a wrong PIN are indistinguishable here on purpose.
    let Some(stored) = stored else {
        return Err(VialoError::AppError(
            StatusCode::FORBIDDEN,
            "invalid_pin".into(),
        ));
    };
    let Ok(stored_pin) = encryption::decrypt::<String>(&stored) else {
        return Err(VialoError::AppError(
            StatusCode::FORBIDDEN,
            "invalid_pin".into(),
        ));
    };
    if !stored_pin.as_bytes().eq(pin.as_bytes()) {
        return Err(VialoError::AppError(
            StatusCode::FORBIDDEN,
            "invalid_pin".into(),
        ));
    }

    let mut conn = grab_authd_conn_subsystem(&data.db, "bookable").await?;
    let res = query!(
        "UPDATE bookable_appointments SET activated = NOW() WHERE id = $1 AND account_id = $2 AND activated IS NULL",
        id,
        record.account_id,
    )
    .execute(&mut *conn)
    .await?;

    if res.rows_affected() != 1 {
        return Err(VialoError::AppError(
            StatusCode::FORBIDDEN,
            "already_activated".into(),
        ));
    }

    Ok(StatusCode::NO_CONTENT)
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Debug, ToSchema, Default)]
pub struct CancelAppointmentBody {
    pub cancellation_reason: Option<CancellationReason>,
    #[serde(default = "yes")]
    pub process_refund: bool,
    pub expected_credits: Option<i32>,
}

/// Cancel appointment
#[utoipa::path(delete, path = "/bookables/appointments/{id}", responses((status = 204, description = "Canceled")))] // no body
pub async fn delete_appointment(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<CancelAppointmentBody>,
) -> Result<impl IntoResponse, VialoError> {
    let owner_check = query!(
        "SELECT ba.account_id, b.asset_type_id FROM bookable_appointments ba JOIN bookable_assets b ON b.id = ba.asset_id WHERE ba.id = $1",
        id
    )
    .fetch_one(&data.db)
    .await?;

    let mut real_cancellation_reason = CancellationReason::User;
    if owner_check.account_id != user.id {
        require_asset_type_perm(
            Some(user.id),
            owner_check.asset_type_id,
            BookablePerm::Admin,
            &data.db,
        )
        .await?;
        real_cancellation_reason = CancellationReason::Admin;
    }

    if real_cancellation_reason != body.cancellation_reason.unwrap_or(CancellationReason::User) {
        return Err(VialoError::AppError(
            StatusCode::BAD_REQUEST,
            "invalid_cancellation_reason".to_string(),
        ));
    };

    // Open transaction and re-read with FOR UPDATE so the read, state check,
    // update, and refund are all atomic to prevent double-refund races.
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let appointment = query!(
        r#"SELECT
          ba.id,
          ba.asset_id,
          ba.transaction_id,
          ba.account_id,
          lower(ba.during) AS begins,
          coalesce(upper(ba.during), '+infinity'::timestamptz) AS "ends!: PgDateTime",
          ba.cancelled_at,
          ba.activated,
          coalesce(cl.credits, 0) as credits,
          cl.from_account,
          ba.maintenance,
          b.asset_type_id,
          COALESCE(p.refund_percent, 0) AS current_refund_percent
        FROM bookable_appointments ba
        JOIN bookable_assets b ON b.id = ba.asset_id
        LEFT JOIN credit_ledger cl ON cl.id = ba.transaction_id
        JOIN LATERAL (
          SELECT DISTINCT ON (asset_id) schema_id, begins
          FROM bookable_schema_assignments
          WHERE asset_id = ba.asset_id
            AND begins <= lower(ba.during)
          ORDER BY asset_id, begins DESC
        ) bsa ON true
        LEFT JOIN LATERAL (
          SELECT refund_percent
          FROM bookable_schema_cancellation_policy
          WHERE schema_id = bsa.schema_id
            AND min_notice <= (lower(ba.during) - now())
          ORDER BY min_notice DESC
          LIMIT 1
        ) p ON true
        WHERE ba.id = $1
        FOR UPDATE OF ba;"#,
        id
    )
    .fetch_one(&mut *trans)
    .await?;

    if appointment.activated.is_some() {
        return Err(VialoError::AppError(
            StatusCode::FORBIDDEN,
            "already_activated".to_string(),
        ));
    }

    if appointment.cancelled_at.is_some() {
        return Err(VialoError::AppError(
            StatusCode::FORBIDDEN,
            "already_cancelled".to_string(),
        ));
    }

    // +inf doesn't count
    if let PgDateTime::DateTime(end_timestamp) = appointment.ends
        && end_timestamp < chrono::Utc::now()
    {
        return Err(VialoError::AppError(
            StatusCode::FORBIDDEN,
            "already_passed".to_string(),
        ));
    }

    if let Some(expected) = body.expected_credits {
        let actual_credits = appointment.credits.unwrap_or(0);
        if actual_credits != expected {
            tracing::error!(
                "expected_credits: expected {} got {}",
                expected,
                actual_credits
            );

            health::add_health_event(
                &data.db,
                Subsystem::App,
                "expected_credits",
                Some(serde_json::json!({
                    "expected": expected,
                    "actual": actual_credits,
                    "appointment_id": id,
                })),
                10,
                false,
                None,
            )
            .await;

            return Err(VialoError::AppError(
                StatusCode::CONFLICT,
                "expected_credits".to_string(),
            ));
        }
    }

    let result = sqlx::query!(
        "UPDATE bookable_appointments SET cancelled_at = now(), cancellation_reason = $2 WHERE id = $1 AND cancelled_at IS NULL AND activated IS NULL",
        appointment.id,
        real_cancellation_reason as CancellationReason
    )
    .execute(&mut *trans).await?;

    if result.rows_affected() == 0 {
        return Err(VialoError::AppError(
            StatusCode::CONFLICT,
            "already_cancelled".to_string(),
        ));
    }

    let refund_credits =
        appointment.credits.unwrap_or(0) * appointment.current_refund_percent.unwrap_or(0) / 100;
    if body.process_refund && refund_credits > 0 {
        query!(
            "INSERT INTO credit_ledger (to_account, refund_of, credits) VALUES ($1, $2, $3)",
            appointment.from_account,
            appointment.transaction_id,
            refund_credits
        )
        .execute(&mut *trans)
        .await?;
    }

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/bookables/appointments", params(
    BookableAppointmentFilterOptions
), responses((status = 200, description = "OK", body=Vec<BookableAppointmentType>)))]
pub async fn list(
    Query(opts): Query<BookableAppointmentFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let _langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    // Permission check in case we receive an account ID different from the current account
    let resolved_account_id = opts.account_id.resolve(user.id);
    if resolved_account_id != IdOrAllQuery::Id(user.id) {
        check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    }

    let record = conditional_query_as!(
        BookableAppointmentType,
        r#"SELECT
            ba.id,
            ba.asset_id,
            ba.transaction_id,
            {#account_info}
            lower(ba.during) as begins,
            coalesce(upper(ba.during), '+infinity'::timestamptz) as "ends!: PgDateTime",
            ba.cancelled_at,
            ba.cancellation_reason as "cancellation_reason: CancellationReason",
            ba.activated,
            ba.maintenance
        FROM
            bookable_appointments ba LEFT JOIN accounts_people ap ON ba.account_id = ap.id LEFT JOIN account_groups ag ON ba.account_id = ag.id WHERE true {#account} {#from} {#to} {#cancelled} ORDER BY during LIMIT {limit} OFFSET {offset}"#,
            #account_info = match(&resolved_account_id){
                  IdOrAllQuery::All => r#"jsonb_build_object('id', ba.account_id, 'full_name', COALESCE(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account: AccountEmbed","#,
                  _ => r#"null AS "account: AccountEmbed","#
            },
            #account = match (&resolved_account_id) {
                IdOrAllQuery::All => "",
                IdOrAllQuery::Id(account) => "AND account_id = {account}",
            },
            #from = match (opts.from) {
                Some(from) => "AND upper(ba.during) >= {from} ",
                None => ""
            },
            #to = match (opts.to) {
                Some(to) => "AND UPPER(ba.during) <= {to} ",
                None => ""
            },
            #cancelled = match (opts.include_cancelled) {
                true => "",
                false => "AND ba.cancelled_at IS NULL",
            }

    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}
