use super::models::{BookableAssetTranslated, BookableStatus};
use super::permissions::{BookablePerm, require_asset_type_perm_by_asset};
use crate::http::util::{JsonE, User, VialoError};
use crate::http::util::{grab_authd_conn_user, grab_trans};
// use crate::ketoapi::subject::Ref;
// use crate::ketoapi::{self, CheckRequest, Subject};
use crate::{AppState, health, http::history::models::Subsystem, impl_jsonb_embed};
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use chrono::{DateTime, Local, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{query, query_as, query_scalar};
use sqlx_conditional_queries::conditional_query_as;
use std::collections::HashMap;
use std::i64;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct TakenSlotQuery {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub asset_id: Option<Vec<i32>>,
    pub asset_type_id: Option<Vec<i32>>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct TakenSlots(HashMap<String, HashMap<String, Vec<i32>>>);
impl_jsonb_embed!(TakenSlots);

#[utoipa::path(get, path = "/bookables/slots/taken", params(TakenSlotQuery), responses((status = 200, description = "OK", body=TakenSlots)))]
pub async fn taken_slots(
    Query(opts): Query<TakenSlotQuery>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let asset_types = opts.asset_type_id.filter(|v| !v.is_empty());
    let asset_ids = opts.asset_id.filter(|v| !v.is_empty());

    let record: TakenSlots = query_scalar!(
        r#"select coalesce(jsonb_object_agg(d.date, d.assets), '{}'::jsonb) as "taken!: TakenSlots" from (
            SELECT * from get_taken_slots(
                (SELECT COALESCE(array_agg(id), ARRAY[]::int[])
                 FROM bookable_assets
                 WHERE ($1::int[] IS NULL OR asset_type_id = ANY($1))
                   AND ($2::int[] IS NULL OR id = ANY($2))),
                $3, $4)
        ) d"#,
        asset_types.as_deref(),
        asset_ids.as_deref(),
        opts.from,
        opts.to
    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, ToSchema)]
pub struct Transition {
    pub schema_id: i32,
    pub begins: NaiveDate,
    pub schedule: Vec<NaiveTime>,
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

#[utoipa::path(get, path = "/bookables/slots/schemas", params(SlotSchemaQuery), responses((status = 200, description = "OK", body=SlotSchemaResponse)))]
pub async fn slot_schemas(
    Query(opts): Query<SlotSchemaQuery>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let asset_types = opts.asset_type_id.filter(|v| !v.is_empty());

    let asset_ids = opts.asset_id.filter(|v| !v.is_empty());
    let assets = conditional_query_as!(
        BookableAssetTranslated,
        r#"SELECT
           bd.id as "id!",
           icon,
           get_i18n_string(bd.name_i18n, {langs: Vec<String>}) AS name,
           bd.asset_type_id as "asset_type_id!",
           status as "status!: BookableStatus"
       FROM
           bookable_asset_status bd WHERE TRUE {#asset_type} {#asset_id}"#,
           #asset_type = match(asset_types){
                 Some(a) => "AND asset_type_id = ANY({a:Vec<i32>})",
                 None => ""
           },
           #asset_id = match(asset_ids.clone()){
                 Some(b) => "AND id = ANY({b:Vec<i32>})",
                 None => ""
           }
    )
    .fetch_all(&data.db)
    .await?;

    let asset_ids: Vec<i32> = asset_ids.unwrap_or_else(|| assets.iter().map(|j| j.id).collect());

    let page_query = query_as!(
        SchemaPages,
        r#"SELECT coalesce(transitions, ARRAY[]::jsonb[]) as "transitions!: Vec<Transition>", coalesce(ARRAY_AGG(asset_id), ARRAY[]::integer[]) as "assets!" FROM (SELECT ARRAY_AGG(jsonb_build_object('schema_id', bsa.schema_id, 'begins', bsa.begins, 'schedule', bs.schedule, 'slot_price', slot_price) ORDER BY bsa.begins) as transitions, bsa.asset_id FROM bookable_schema_assignments bsa JOIN bookable_schemas bs ON bs.id = bsa.schema_id WHERE bsa.asset_id = ANY($1) AND bsa.begins <= $2 AND (bsa.begins > $3 OR bsa.begins = (SELECT MAX(begins) FROM bookable_schema_assignments WHERE asset_id = bsa.asset_id AND begins <= $3)) group by bsa.asset_id) group by transitions;"#,
        &asset_ids,
        opts.to,
        opts.from
    )
    .fetch_all(&data.db);

    if let Ok(pages) = page_query.await {
        // let mut evil: HashMap<Vec<i32>, Vec<&SchemaAssignments>> = HashMap::new();
        // for record in schema_assignments.iter() {
        //     if let Some(key_real) = &record.assets {
        //         match evil.get_mut::<Vec<i32>>(key_real) {
        //             Some(v) => {
        //                 v.push(record);
        //             }
        //             None => {
        //                 evil.insert(key_real.clone(), vec![record]);
        //             }
        //         }
        //     }
        // }

        return Ok((StatusCode::OK, Json(SlotSchemaResponse { pages, assets })));
    }

    Err(VialoError::NotFound())
}

#[derive(Serialize, Deserialize, Debug, Clone, ToSchema)]
pub struct BookSlotSchemaSlot {
    pub asset_id: i32,
    pub slot_index: u16,
    pub expected_start: DateTime<Utc>,
    pub expected_end: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct BookSlotSchema {
    pub expected_sum_total: i32,
    pub slots: Vec<BookSlotSchemaSlot>,
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
}

#[utoipa::path(post, path = "/bookables/slots/book", request_body = BookSlotSchema, responses((status = 200, description = "Booked", body=Vec<MaterializedSlots>),(status = 400, description = "Slot expectation failed"),(status = 400, description = "Slot index failure"),(status = 400, description = "Slot info failure")))]
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

    // Materialize the slots (Give them concrete start and end points)
    let materialized_slots = query_as!(MaterializedSlots, r#"SELECT DISTINCT ON (j_i) bsa.schema_id, (j_i-1) as index, (((j_v->>'expected_start')::timestamptz AT TIME ZONE current_setting('TIMEZONE'))::date + bs.schedule[(j_v->>'slot_index')::int+1])::timestamptz as begins, (((j_v->>'expected_start')::timestamptz AT TIME ZONE current_setting('TIMEZONE'))::date + bs.schedule[(j_v->>'slot_index')::int+2])::timestamptz as ends, bs.slot_price as price, (j_v->>'asset_id')::int as asset_id
        FROM bookable_schema_assignments bsa
        JOIN jsonb_array_elements($1) WITH ORDINALITY AS j(j_v, j_i) ON bsa.begins <= ((j_v->>'expected_start')::timestamptz AT TIME ZONE current_setting('TIMEZONE'))::date AND bsa.asset_id = (j_v->>'asset_id')::int
        JOIN bookable_schemas bs ON bsa.schema_id = bs.id
        ORDER BY j_i, bsa.begins DESC;"#, json!(body.slots)).fetch_all(&data.db)
        .await?;

    tracing::debug!("materialized_slots: {:?}", materialized_slots);

    // Verify the slots and the client's expectations (if present)
    let mut sum_total = 0;
    for slot in &materialized_slots {
        let idx: usize = slot.index.ok_or(VialoError::AppError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "slot_info".to_string(),
        ))? as usize;
        let expected_slot: &BookSlotSchemaSlot = body.slots.get(idx).ok_or(
            VialoError::AppError(StatusCode::BAD_REQUEST, "slot_index".to_string()),
        )?;
        if let (Some(real_start), Some(real_end)) = (slot.begins, slot.ends) {
            if let Some(expected_end) = expected_slot.expected_end {
                if expected_slot.expected_start != real_start || expected_end != real_end {
                    tracing::debug!(
                        "expected_start: {:?}, real_start: {:?}, expected_end: {:?}, real_end: {:?}",
                        expected_slot.expected_start,
                        real_start,
                        expected_end,
                        real_end
                    );
                    return Err(VialoError::AppError(
                        StatusCode::BAD_REQUEST,
                        "slot_expectation".to_string(),
                    ));
                }
            }
        } else {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "slot_index".to_string(),
            ));
        }
        if let Some(price) = slot.price {
            sum_total += price;
        }
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

        return Err(VialoError::AppError(
            StatusCode::BAD_REQUEST,
            "sum_total_expectation".to_string(),
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

    if balance.is_some_and(|b| b < sum_total) {
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
            SELECT tx_id, 'bookable', $1, (slot->>'price')::int, NOW()
            FROM input
        )
        INSERT INTO bookable_appointments (asset_id, account_id, during, transaction_id)
        SELECT (slot->>'asset_id')::int,
               $1,
               tstzrange((slot->>'begins')::timestamptz, (slot->>'ends')::timestamptz, '[)'),
               tx_id
        FROM input;"#,
        user.id,
        json!(materialized_slots),
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

    // Broadcast updated status (only for slots that start now i.e. change status immediately)
    {
        let now = Utc::now();
        let asset_ids: std::collections::BTreeSet<i32> = body
            .slots
            .iter()
            .filter(|s| s.expected_start <= now)
            .map(|s| s.asset_id)
            .collect();
        if !asset_ids.is_empty() {
            let evil_data = data.clone();
            tokio::spawn(async move {
                crate::bookables::fetch_and_broadcast_status(&evil_data, asset_ids).await;
            });
        }
    }

    Ok((StatusCode::OK, Json(materialized_slots)))
}
