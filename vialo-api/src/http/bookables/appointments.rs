use super::permissions::{
    AssetTypeScope, BookableCaller, BookablePerm, require_asset_type_perm, resolve_asset_type_scope,
};
use crate::bookables::CancellationReason;
use crate::helpers::{PgDateTime, encryption, grab_authd_conn_subsystem};
use crate::http::history::models::Subsystem;
use crate::http::people::models::TransactionStatus;
use crate::http::util::models::{AccountEmbed, IdOrAllQuery, IdOrMeOrAllQuery};
use crate::http::util::{
    MaybeJsonE, User, VialoError, clamp_pagination, grab_authd_conn_user, grab_trans,
};
use crate::{AppState, health, impl_jsonb_embed};
use axum::extract::{ConnectInfo, Path};
use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::Query;
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
    pub asset_types: Option<Vec<i32>>,
    #[serde(default)]
    pub include_cancelled: bool,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct TransactionEmbed {
    pub id: Uuid,
    pub credits: Option<i32>,
    pub credits_refunded: i32,
    pub from_account_id: Option<Uuid>,
    pub to_account_id: Option<Uuid>,
    pub status: TransactionStatus,
    pub created_at: DateTime<Utc>,
}

impl_jsonb_embed!(TransactionEmbed);

#[derive(Serialize, ToSchema)]
pub struct BookableAppointmentModel {
    pub id: Uuid,
    pub asset_id: i32,
    pub transaction_id: Option<Uuid>,
    pub account: Option<AccountEmbed>,

    pub transaction: Option<TransactionEmbed>,
    pub refund_transactions: Vec<TransactionEmbed>,

    pub begins: Option<DateTime<Utc>>,
    pub ends: PgDateTime,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub cancellation_reason: Option<CancellationReason>,
    #[schema(format = DateTime)]
    pub activated: Option<chrono::DateTime<chrono::Utc>>,
    pub maintenance: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct BookableAppointmentListModel {
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
    headers: HeaderMap,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    caller: BookableCaller,
    body: Option<Json<KioskActivateBody>>,
) -> Result<impl IntoResponse, VialoError> {
    let user_id = match caller {
        // A valid kiosk credential: activate on behalf of the appointment's owner
        BookableCaller::Kiosk(kiosk) => {
            let Some(Json(body)) = body else {
                return Err(VialoError::Forbidden());
            };
            let Some(pin) = body.pin else {
                return Err(VialoError::Forbidden());
            };

            // The PIN is brute-forceable, so we throttle it per IP.
            data.rate_limiters
                .check_kiosk_pin(&headers, connect_info.as_deref())?;

            return kiosk_activate(&data, id, &kiosk, &pin).await;
        }
        BookableCaller::Account(user_id) => user_id,
        BookableCaller::Anonymous => {
            return Err(VialoError::Forbidden());
        }
    };

    // Implicit permission check in the query
    let mut conn = grab_authd_conn_user(&data.db, user_id).await?;

    let res = query!(
        "UPDATE bookable_appointments SET activated = NOW() WHERE id = $1 AND account_id = $2",
        id,
        user_id
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

#[derive(Serialize, Deserialize, Debug, ToSchema, Default)]
pub struct CancelAppointmentBody {
    pub cancellation_reason: Option<CancellationReason>,
    /// Defaults to true
    pub process_refund: Option<bool>,
    pub expected_credits: Option<i32>,
}

/// Cancel appointment
#[utoipa::path(post, path = "/bookables/appointments/{id}/cancel", request_body = CancelAppointmentBody, responses((status = 204, description = "Canceled")))] // no body
pub async fn delete_appointment(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    MaybeJsonE(body): MaybeJsonE<CancelAppointmentBody>,
) -> Result<impl IntoResponse, VialoError> {
    let body = body.unwrap_or_default();

    let owner_check = query!(
        "SELECT ba.account_id, b.asset_type_id FROM bookable_appointments ba JOIN bookable_assets b ON b.id = ba.asset_id WHERE ba.id = $1",
        id
    )
    .fetch_one(&data.db)
    .await?;

    let mut is_admin = false;
    if owner_check.account_id != user.id {
        require_asset_type_perm(
            Some(user.id),
            owner_check.asset_type_id,
            BookablePerm::Admin,
            &data.db,
        )
        .await?;
        is_admin = true;
    }

    if is_admin && body.cancellation_reason != Some(CancellationReason::Admin) {
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
          coalesce(cl.credits - cl.credits_refunded, 0) AS refundable_credits,
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

    let process_refund = body.process_refund.unwrap_or(true);

    // The policy is a percentage of the original price, but an admin undo may already have
    // returned part of it. Never hand back more than is left.
    let refund_credits =
        (appointment.credits.unwrap_or(0) * appointment.current_refund_percent.unwrap_or(0) / 100)
            .min(appointment.refundable_credits.unwrap_or(0));

    if let Some(expected) = body.expected_credits
        && process_refund
        && refund_credits != expected
    {
        tracing::error!(
            "expected_credits: expected {} got {}",
            expected,
            refund_credits
        );

        let details = serde_json::json!({
            "expected": expected,
            "actual": refund_credits,
            "appointment_id": id,
        });
        health::add_health_event(
            &data.db,
            Subsystem::App,
            "expected_credits",
            Some(&details),
            10,
            false,
            None,
        )
        .await;

        return Err(VialoError::AppErrorWithDetails(
            StatusCode::CONFLICT,
            "expected_credits".to_string(),
            details,
        ));
    }

    let result = sqlx::query!(
        "UPDATE bookable_appointments SET cancelled_at = now(), cancellation_reason = $2 WHERE id = $1 AND cancelled_at IS NULL AND activated IS NULL",
        appointment.id,
        body.cancellation_reason.unwrap_or(CancellationReason::User) as CancellationReason
    )
    .execute(&mut *trans).await?;

    if result.rows_affected() == 0 {
        return Err(VialoError::AppError(
            StatusCode::CONFLICT,
            "already_cancelled".to_string(),
        ));
    }

    if process_refund && refund_credits > 0 {
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
), responses((status = 200, description = "OK", body=Vec<BookableAppointmentListModel>)))]
pub async fn list(
    Query(opts): Query<BookableAppointmentFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let _langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let user_id = user.id;
    let resolved_account_id = opts.account_id.resolve(user.id);
    // Permission filter
    let scope = resolve_asset_type_scope(user_id, BookablePerm::Admin, &data.db).await?;

    let record = conditional_query_as!(
        BookableAppointmentListModel,
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
            bookable_appointments ba JOIN bookable_assets b ON b.id = ba.asset_id LEFT JOIN accounts_people ap ON ba.account_id = ap.id LEFT JOIN account_groups ag ON ba.account_id = ag.id WHERE true
            {#permission} {#account} {#asset_types} {#from} {#to} {#cancelled} ORDER BY ba.during, ba.id LIMIT {limit} OFFSET {offset}"#,
            #account_info = match(&resolved_account_id){
                  IdOrAllQuery::All => r#"jsonb_build_object('id', ba.account_id, 'full_name', COALESCE(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account: AccountEmbed","#,
                  _ => r#"null AS "account: AccountEmbed","#
            },
            #permission = match (scope) {
                AssetTypeScope::All => "",
                AssetTypeScope::Only(scope_asset_types) => "AND (ba.account_id = {user_id} OR b.asset_type_id = ANY({scope_asset_types:Vec<i32>}))",
                AssetTypeScope::None => "AND ba.account_id = {user_id}",
            },
            #account = match (&resolved_account_id) {
                IdOrAllQuery::All => "",
                IdOrAllQuery::Id(account) => "AND ba.account_id = {account}",
            },
            #asset_types = match (opts.asset_types) {
                Some(at) => "AND b.asset_type_id = ANY({at:Vec<i32>})",
                None => "",
            },
            #from = match (opts.from) {
                Some(from) => "AND coalesce(upper(ba.during), 'infinity'::timestamptz) >= {from} ",
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

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct BookableAppointmentGetOptions {
    #[serde(default)]
    pub account_info: bool,
    #[serde(default)]
    pub transaction_info: bool,
}

#[utoipa::path(get, path = "/bookables/appointments/{id}", params(
    BookableAppointmentGetOptions
), responses((status = 200, description = "OK", body=BookableAppointmentModel)))]
pub async fn get(
    Query(opts): Query<BookableAppointmentGetOptions>,
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let user_id = user.id;

    let record = conditional_query_as!(
        BookableAppointmentModel,
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
            ba.maintenance,
            {#transaction_info}
        FROM
            bookable_appointments ba JOIN bookable_assets b ON b.id = ba.asset_id
            LEFT JOIN accounts_people ap ON ba.account_id = ap.id LEFT JOIN account_groups ag ON ba.account_id = ag.id
            LEFT JOIN credit_ledger cl ON ba.transaction_id = cl.id
            LEFT JOIN LATERAL (
              SELECT array_agg(
                jsonb_build_object('id', r.id, 'from_account_id', r.from_account, 'to_account_id', r.to_account, 'credits', r.credits, 'credits_refunded', r.credits_refunded, 'created_at', r.created_at, 'status', r.status)
                ORDER BY r.created_at
              ) AS refunds
              FROM credit_ledger r WHERE r.refund_of = cl.id
            ) clr ON true
            WHERE ba.id = {id} AND (ba.account_id = {user_id} OR account_bookable_perm_exists({user_id}, b.asset_type_id, 'admin'::bookable_perm))"#,
            #account_info = match(opts.account_info){
                 true => r#"jsonb_build_object('id', ba.account_id, 'full_name', COALESCE(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account: AccountEmbed","#,
                false => r#"null AS "account: AccountEmbed","#
            },
            #transaction_info= match(opts.transaction_info){
                true => r#"CASE WHEN cl.id IS NOT NULL THEN jsonb_build_object('id', cl.id, 'from_account_id', cl.from_account, 'to_account_id', cl.to_account, 'credits', cl.credits, 'credits_refunded', cl.credits_refunded, 'created_at', cl.created_at, 'status', cl.status) ELSE null END AS "transaction?: TransactionEmbed",
                        COALESCE(clr.refunds, ARRAY[]::jsonb[]) AS "refund_transactions!: Vec<TransactionEmbed>""#,
                false => r#"null AS "transaction: TransactionEmbed",
                        ARRAY[]::jsonb[] AS "refund_transactions!: Vec<TransactionEmbed>""#,
            }

    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}
