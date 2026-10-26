use super::appointments::{default_owner, resolve_owner};
use super::models::{BookableAssetTranslated, BookableStatus};
use super::permissions::{BookablePerm, require_asset_type_perm_by_asset};
use crate::http::util::models::IdOrMeQuery;
use crate::http::util::{JsonE, User, VialoError};
use crate::http::util::{grab_authd_conn_user, grab_trans};
// use crate::ketoapi::subject::Ref;
// use crate::ketoapi::{self, CheckRequest, Subject};
use crate::{AppState, health, http::history::models::Subsystem, impl_jsonb_embed};
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::types::PgRange;
use sqlx::{query, query_as, query_scalar};
use sqlx_conditional_queries::conditional_query_as;
use std::collections::HashMap;
use std::ops::Bound;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// Longest from..=to window the slot endpoints serve, page with consecutive windows beyond that
const MAX_SLOT_WINDOW_DAYS: i64 = 31;

fn check_slot_window(from: NaiveDate, to: NaiveDate) -> Result<(NaiveDate, NaiveDate), VialoError> {
    if !(1..=MAX_SLOT_WINDOW_DAYS).contains(&((to - from).num_days() + 1)) {
        return Err(VialoError::AppError(
            StatusCode::BAD_REQUEST,
            format!("from..=to must span 1 to {} days", MAX_SLOT_WINDOW_DAYS),
        ));
    }

    Ok((from, to))
}

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct TakenSlotQuery {
    pub from: NaiveDate,
    /// Inclusive, at most 31 days including `from`
    pub to: NaiveDate,
    pub asset_id: Option<Vec<i32>>,
    pub asset_type_id: Option<Vec<i32>>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct TakenSlots(HashMap<String, HashMap<String, Vec<i32>>>);
impl_jsonb_embed!(TakenSlots);

#[utoipa::path(get, path = "/bookables/slots/taken", params(TakenSlotQuery), responses((status = 200, description = "OK", body=TakenSlots),(status = 400, description = "from..=to is reversed or longer than 31 days")))]
pub async fn taken_slots(
    Query(opts): Query<TakenSlotQuery>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    let (from, to) = check_slot_window(opts.from, opts.to)?;
    let asset_types = opts.asset_type_id.filter(|v| !v.is_empty());
    let asset_ids = opts.asset_id.filter(|v| !v.is_empty());

    let record: TakenSlots = query_scalar!(
        r#"select coalesce(jsonb_object_agg(d.date, d.assets), '{}'::jsonb) as "taken!: TakenSlots" from (
            SELECT * from get_taken_slots(
                (SELECT COALESCE(array_agg(id), ARRAY[]::int[])
                 FROM bookable_assets
                 WHERE ($1::int[] IS NULL OR asset_type_id = ANY($1))
                   AND ($2::int[] IS NULL OR id = ANY($2))
                   AND account_bookable_perm_exists($5, asset_type_id, 'view'::bookable_perm)),
                $3, $4)
        ) d"#,
        asset_types.as_deref(),
        asset_ids.as_deref(),
        from,
        to,
        user.id
    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, ToSchema)]
pub struct Transition {
    pub schema_id: i32,
    pub begins: NaiveDate,
    /// strings, because `time` allows 24:00:00 and `NaiveTime` doesn't >:(
    pub schedule: Vec<String>,
    pub slot_price: i32,
}

impl_jsonb_embed!(Transition);

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SchemaPages {
    pub transitions: Vec<Transition>,
    pub assets: Vec<i32>,
}
#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct SlotSchemaQuery {
    pub from: NaiveDate,
    /// Inclusive, at most 31 days including `from`
    pub to: NaiveDate,
    pub asset_id: Option<Vec<i32>>,
    pub asset_type_id: Option<Vec<i32>>,
    pub lang: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema)]
pub struct SlotSchemaResponse {
    pub pages: Vec<SchemaPages>,
    pub assets: Vec<BookableAssetTranslated>,
}

#[utoipa::path(get, path = "/bookables/slots/schemas", params(SlotSchemaQuery), responses((status = 200, description = "OK", body=SlotSchemaResponse),(status = 400, description = "from..=to is reversed or longer than 31 days")))]
pub async fn slot_schemas(
    Query(opts): Query<SlotSchemaQuery>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    let (from, to) = check_slot_window(opts.from, opts.to)?;
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let asset_types = opts.asset_type_id.filter(|v| !v.is_empty());

    let asset_ids = opts.asset_id.filter(|v| !v.is_empty());
    let user_id = user.id;
    let assets = conditional_query_as!(
        BookableAssetTranslated,
        r#"SELECT
           bd.id as "id!",
           icon,
           get_i18n_string(bd.name_i18n, {langs: Vec<String>}) AS name,
           bd.asset_type_id as "asset_type_id!",
           status as "status!: BookableStatus"
       FROM
           bookable_asset_status bd
       WHERE account_bookable_perm_exists({user_id}, bd.asset_type_id, 'view'::bookable_perm)
       {#asset_type} {#asset_id}"#,
           #asset_type = match(asset_types){
                 Some(a) => "AND asset_type_id = ANY({a:Vec<i32>})",
                 None => ""
           },
           #asset_id = match(asset_ids){
                 Some(b) => "AND id = ANY({b:Vec<i32>})",
                 None => ""
           }
    )
    .fetch_all(&data.db)
    .await?;

    // Reuse allowed assets
    let asset_ids: Vec<i32> = assets.iter().map(|j| j.id).collect();

    let pages = query_as!(
        SchemaPages,
        r#"SELECT coalesce(transitions, ARRAY[]::jsonb[]) as "transitions!: Vec<Transition>", coalesce(ARRAY_AGG(asset_id), ARRAY[]::integer[]) as "assets!" FROM (SELECT ARRAY_AGG(jsonb_build_object('schema_id', bsa.schema_id, 'begins', bsa.begins, 'schedule', bs.schedule, 'slot_price', slot_price) ORDER BY bsa.begins) as transitions, bsa.asset_id FROM bookable_schema_assignments bsa JOIN bookable_schemas bs ON bs.id = bsa.schema_id WHERE bsa.asset_id = ANY($1) AND bsa.begins <= $2 AND (bsa.begins > $3 OR bsa.begins = (SELECT MAX(begins) FROM bookable_schema_assignments WHERE asset_id = bsa.asset_id AND begins <= $3)) group by bsa.asset_id) group by transitions;"#,
        &asset_ids,
        to,
        from
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(SlotSchemaResponse { pages, assets })))
}

#[derive(Serialize, Deserialize, Debug, Clone, ToSchema)]
pub struct BookSlotSchemaSlot {
    pub asset_id: i32,
    pub date: NaiveDate,
    pub slot_index: u16,
    /// Wall-clock bounds as the schedule showed them, e.g. "12:00:00". Rejected if it's changed since
    pub expected_start: String,
    pub expected_end: String,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct BookSlotSchema {
    /// 0 unless booking for yourself
    pub expected_sum_total: i32,
    pub slots: Vec<BookSlotSchemaSlot>,
    /// Defaults to me, anyone else (or null, held by the asset type's admins) needs admin and is free
    #[serde(default = "default_owner", skip_serializing)]
    #[schema(value_type = Option<IdOrMeQuery>)]
    pub account_id: Option<IdOrMeQuery>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct MaterializedSlots {
    pub schema_id: i32,
    pub index: Option<i64>,
    #[schema(format = DateTime)]
    pub begins: Option<DateTime<Utc>>,
    #[schema(format = DateTime)]
    pub ends: Option<DateTime<Utc>>,
    pub asset_id: Option<i32>,
    pub price: Option<i32>,
    pub existing_expired: Option<Uuid>,
    /// Whether slot_index is in the schedule at all, to tell a bad index from a slot skipped that day
    #[serde(skip)]
    pub in_schedule: Option<bool>,
    /// Whether the schedule still has the bounds the client saw
    #[serde(skip)]
    pub matches_expectation: Option<bool>,
}

#[utoipa::path(post, path = "/bookables/slots/book", request_body = BookSlotSchema, responses((status = 200, description = "Booked", body=Vec<MaterializedSlots>),(status = 400, description = "Slot expectation failed"),(status = 400, description = "Slot index failure"),(status = 400, description = "Slot info failure"),(status = 400, description = "Slot skipped by a DST change that day")))]
pub async fn book_slots(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookSlotSchema>,
) -> Result<impl IntoResponse, VialoError> {
    // Check book permission on every asset being booked
    let asset_ids: Vec<i32> = body.slots.iter().map(|s| s.asset_id).collect();
    for asset_id in &asset_ids {
        require_asset_type_perm_by_asset(user.id, *asset_id, BookablePerm::Book, &data.db).await?;
    }
    let (owner, paid) = resolve_owner(&data.db, user.id, &body.account_id, &asset_ids).await?;

    // Materialize the slots (Give them concrete start and end points). Through
    // slots_from_schedule, so they match get_taken_slots across DST changes.
    let mut materialized_slots = query_as!(MaterializedSlots, r#"SELECT DISTINCT ON (j_i) bsa.schema_id, (j_i-1) as index,
            lower(sl.range) as begins,
            upper(sl.range) as ends,
            bs.slot_price as price, (j_v->>'asset_id')::int as asset_id,
            NULL as "existing_expired: Uuid",
            (j_v->>'slot_index')::int + 2 <= cardinality(bs.schedule) as in_schedule,
            coalesce(bs.schedule[(j_v->>'slot_index')::int + 1]::text = j_v->>'expected_start'
                AND bs.schedule[(j_v->>'slot_index')::int + 2]::text = j_v->>'expected_end', false) as matches_expectation
        FROM bookable_schema_assignments bsa
        JOIN jsonb_array_elements($1) WITH ORDINALITY AS j(j_v, j_i) ON bsa.begins <= (j_v->>'date')::date AND bsa.asset_id = (j_v->>'asset_id')::int
        JOIN bookable_schemas bs ON bsa.schema_id = bs.id
        LEFT JOIN LATERAL slots_from_schedule(bs.schedule, (j_v->>'date')::date) sl ON sl.id = (j_v->>'slot_index')::int
        ORDER BY j_i, bsa.begins DESC;"#, json!(body.slots)).fetch_all(&data.db)
        .await?;

    // Verify the slots and the client's expectations
    let mut sum_total = 0;

    // Check for existing expired slots from this user
    let existing_expired = query_scalar!(
        r#"SELECT DISTINCT ON (j_i) ba.id as "id?" FROM UNNEST($2::tstzrange[]) WITH ORDINALITY AS j(j_v, j_i) LEFT JOIN bookable_appointments ba ON ba.account_id = $1 AND cancellation_reason = 'expired' AND during = j_v ORDER BY j_i"#,
        user.id,
        &materialized_slots
            .iter()
            .map(|s| PgRange {
                start: s
                    .begins
                    .map(|d| Bound::Included(d))
                    .unwrap_or(Bound::Unbounded),
                end: s
                    .ends
                    .map(|d| Bound::Excluded(d))
                    .unwrap_or(Bound::Unbounded),
            })
            .collect::<Vec<_>>()
    ).fetch_all(&data.db)
    .await?;

    for (slot, expired_slot) in materialized_slots.iter_mut().zip(existing_expired) {
        let idx: usize = slot.index.ok_or(VialoError::AppError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "slot_info".to_string(),
        ))? as usize;
        let expected_slot: &BookSlotSchemaSlot = body.slots.get(idx).ok_or(
            VialoError::AppError(StatusCode::BAD_REQUEST, "slot_index".to_string()),
        )?;
        if slot.in_schedule != Some(true) {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "slot_index".to_string(),
            ));
        }
        if slot.matches_expectation != Some(true) {
            tracing::debug!(
                "expected {}..{} at slot {} on {}, schema {} says otherwise",
                expected_slot.expected_start,
                expected_slot.expected_end,
                expected_slot.slot_index,
                expected_slot.date,
                slot.schema_id
            );
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "slot_expectation".to_string(),
            ));
        }
        let Some(real_end) = slot.ends else {
            // Swallowed by a spring-forward gap, see slots_from_schedule
            return Err(VialoError::AppErrorWithDetails(
                StatusCode::BAD_REQUEST,
                "slot_nonexistent".to_string(),
                json!(slot),
            ));
        };
        if real_end < chrono::Utc::now() {
            return Err(VialoError::AppErrorWithDetails(
                StatusCode::BAD_REQUEST,
                "slot_has_passed".to_string(),
                json!(slot),
            ));
        }

        if let Some(price) = slot.price
            && paid
            && expired_slot.is_none()
        {
            sum_total += price;
        }

        slot.existing_expired = expired_slot;
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    if sum_total != body.expected_sum_total {
        tracing::error!(
            "sum_total_expectation: expected {} got {}",
            body.expected_sum_total,
            sum_total
        );

        health::add_health_event(
            &mut *conn,
            Subsystem::App,
            "sum_total_expectation",
            Some(serde_json::json!({
                "expected": body.expected_sum_total,
                "actual": sum_total,
                "slots": body.slots
            })),
            10,
            false,
            None,
        )
        .await;

        return Err(VialoError::AppErrorWithDetails(
            StatusCode::BAD_REQUEST,
            "sum_total_expectation".to_string(),
            json!({
                "expected": body.expected_sum_total,
                "actual": sum_total,
                "slots": body.slots
            }),
        ));
    }

    let mut trans = grab_trans(&mut conn).await?;

    // Check balance (NULL means unlimited credits)
    let balance = query_scalar!(
        "SELECT credit_balance FROM accounts_people WHERE id = $1 FOR UPDATE",
        user.id
    )
    .fetch_one(&mut *trans)
    .await?;

    if paid && balance.is_some_and(|b| b < sum_total) {
        return Err(VialoError::AppError(
            StatusCode::PAYMENT_REQUIRED,
            "insufficient_credits".into(),
        ));
    }

    // The materialized slots seem to be right. Let's insert everything.
    let slot_insert_result = query!(
        r#"WITH input AS MATERIALIZED (
            SELECT j AS slot, gen_random_uuid() AS tx_id
            FROM jsonb_array_elements($2) AS j
        ),
        ins_ledger AS (
            INSERT INTO credit_ledger (id, product, from_account, credits, created_at)
            -- re-picking your own expired slot is free
            SELECT tx_id, 'bookable', $1, CASE WHEN slot->>'existing_expired' IS NULL THEN (slot->>'price')::int ELSE 0 END, NOW()
            FROM input
            WHERE $4
        )
        INSERT INTO bookable_appointments (asset_id, account_id, during, transaction_id)
        SELECT (slot->>'asset_id')::int,
               $3,
               tstzrange((slot->>'begins')::timestamptz, (slot->>'ends')::timestamptz, '[)'),
               CASE WHEN $4 THEN tx_id END
        FROM input;"#,
        user.id,
        json!(materialized_slots),
        owner,
        paid,
    )
    .execute(&mut *trans)
    .await;

    if let Err(insert_error) = slot_insert_result {
        if let sqlx::Error::Database(db_err) = &insert_error
            && let Some(constraint) = db_err.constraint()
            && constraint == "no_overlapping_appointments_per_asset"
        {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "overlap".to_string(),
            ));
        };
        return Err(VialoError::Anyhow(insert_error.into()));
    }

    trans.commit().await?;

    Ok((StatusCode::OK, Json(materialized_slots)))
}
