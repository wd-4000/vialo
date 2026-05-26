use super::models::{BookableAssetTranslated, BookableStatus};
use crate::http::util::{JsonE, User, VialoError};
use crate::http::util::{grab_authd_conn_user, grab_trans};
// use crate::ketoapi::subject::Ref;
// use crate::ketoapi::{self, CheckRequest, Subject};
use crate::{AppState, health, http::history::models::Subsystem, impl_jsonb_embed};
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::JsonValue;
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
    pub asset_id: Vec<i32>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct TakenSlots(HashMap<String, HashMap<String, Vec<i32>>>);
impl_jsonb_embed!(TakenSlots);

#[utoipa::path(get, path = "/bookables/slots/taken", params(TakenSlotQuery), responses((status = 200, description = "OK", body=TakenSlots)))]
pub async fn taken_slots(
    Query(opts): Query<TakenSlotQuery>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let record: TakenSlots = query_scalar!(
        r#"select coalesce(jsonb_object_agg(d.date, d.assets), '{}'::jsonb) as "taken!: TakenSlots" from (
            SELECT * from get_taken_slots($1, $2, $3)
        ) d"#,
        &opts.asset_id,
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
    pub begins: DateTime<Utc>,
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

    let asset_types = if opts.asset_type_id.clone().unwrap_or(vec![]).is_empty() {
        None
    } else {
        opts.asset_type_id.clone()
    };

    let asset_ids = if opts.asset_id.clone().unwrap_or(vec![]).is_empty() {
        None
    } else {
        opts.asset_id.clone()
    };
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
        r#"SELECT coalesce(transitions, ARRAY[]::jsonb[]) as "transitions!: Vec<Transition>", coalesce(ARRAY_AGG(asset_id), ARRAY[]::integer[]) as "assets!" FROM (SELECT ARRAY_AGG(jsonb_build_object('schema_id', bsa.schema_id, 'begins', bsa.begins, 'schedule', bs.schedule, 'slot_price', slot_price) ORDER BY bsa.begins) as transitions, bsa.asset_id FROM bookable_schema_assignments bsa JOIN bookable_schemas bs ON bs.id = bsa.schema_id WHERE bsa.asset_id = ANY($1) AND bsa.begins <= $2::date group by bsa.asset_id) group by transitions;"#,
        &asset_ids,
        opts.to
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
    pub expected_start: DateTime<Local>,
    pub expected_end: Option<DateTime<Local>>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct BookSlotSchema {
    pub expected_sum_total: i32,
    pub slots: Vec<BookSlotSchemaSlot>,
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct MaterializedSlots {
    pub schema_id: i32,
    pub index: Option<i64>,
    #[schema(format = DateTime)]
    pub begins: Option<NaiveDateTime>,
    #[schema(format = DateTime)]
    pub ends: Option<NaiveDateTime>,
    pub asset_id: Option<i32>,
    pub price: Option<i32>,
}

#[utoipa::path(post, path = "/bookables/slots/book", request_body = BookSlotSchema, responses((status = 200, description = "Booked", body=Vec<MaterializedSlots>),(status = 400, description = "Slot expectation failed"),(status = 400, description = "Slot index failure"),(status = 400, description = "Slot info failure")))]
pub async fn book_slots(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookSlotSchema>,
) -> Result<impl IntoResponse, VialoError> {
    // Materialize the slots (Give them concrete start and end points)
    let materialized_slots = query_as!(MaterializedSlots, r#"SELECT DISTINCT ON (j_i) bsa.schema_id, (j_i-1) as index, ((j_v->>'expected_start')::date + bs.schedule[(j_v->>'slot_index')::int+1]) as begins, ((j_v->>'expected_start')::date + bs.schedule[(j_v->>'slot_index')::int+2]) as ends, bs.slot_price as price, (j_v->>'asset_id')::int as asset_id
        FROM bookable_schema_assignments bsa
        JOIN jsonb_array_elements($1) WITH ORDINALITY AS j(j_v, j_i) ON bsa.begins <= (j_v->>'expected_start')::date AND bsa.asset_id = (j_v->>'asset_id')::int
        JOIN bookable_schemas bs ON bsa.schema_id = bs.id
        ORDER BY j_i, bsa.begins DESC;"#, json!(body.slots)).fetch_all(&data.db)
        .await?;

    // Verify the slots and the client's expectations (if present)
    let mut sum_total = 0;
    for slot in &materialized_slots {
        let expected_slot: &BookSlotSchemaSlot = &body.slots[slot.index.ok_or(
            VialoError::AppError(StatusCode::INTERNAL_SERVER_ERROR, "slot_info".to_string()),
        )? as usize];
        if let (Some(real_start), Some(real_end)) = (slot.begins, slot.ends) {
            if let Some(expected_end) = expected_slot.expected_end.map(|v| v.naive_local()) {
                println!("{:?}", expected_slot.expected_start.naive_local());
                println!("{:?}", real_start);
                if expected_slot.expected_start.naive_local() != real_start
                    || expected_end != real_end
                {
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
        )
        .await
        .map_err(VialoError::Anyhow)?;

        return Err(VialoError::AppError(
            StatusCode::BAD_REQUEST,
            "sum_total_expectation".to_string(),
        ));
    }

    let mut trans = grab_trans(&mut conn).await?;

    // The materialized slots seem to be right.
    query!(
        "INSERT INTO credit_ledger (
            from_account, credits, created_at
            )
            SELECT
                $1,
                (j->>'price')::int,
                NOW()
            FROM jsonb_array_elements($2) AS j RETURNING id;",
        user.id,
        json!(materialized_slots)
    )
    .fetch_all(&mut *trans)
    .await?;

    let slot_insert_result = query!(
        "INSERT INTO bookable_appointments (
        asset_id,
        account_id,
        during
        )
        SELECT
            (j->>'asset_id')::int,
            $1,
            tsrange((j->>'begins')::timestamp, (j->>'ends')::timestamp, '[)')
        FROM jsonb_array_elements($2) AS j;",
        user.id,
        json!(materialized_slots),
    )
    .fetch_all(&mut *trans)
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
