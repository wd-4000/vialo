use super::models::BoardPostIdModel;
use super::permissions::{
    BookablePerm, require_asset_type_perm, require_asset_type_perm_by_schema,
};
use crate::AppState;
use crate::http::util::grab_authd_conn_user;
use crate::http::util::{JsonE, SearchableListOptions, User, VialoError, clamp_pagination};
use axum::extract::Path;
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use serde_with::{DurationSeconds, serde_as};
use sqlx::postgres::types::PgInterval;
use sqlx::query;
use sqlx_conditional_queries::conditional_query_as;
use std::i64;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct BookableSchema {
    pub id: i32,
    pub label: Option<String>,
    pub schedule: Vec<String>,
    pub asset_type_id: i32,
    pub slot_price: Option<i32>,
    pub activation_grace_period: Option<i64>,
    pub expiry_refund_percent: Option<i16>,
}

#[utoipa::path(get, path = "/bookables/schemas", params(SearchableListOptions), responses((status = 200, description = "OK", body=Vec<BookableSchema>)))]
pub async fn list(
    Query(opts): Query<SearchableListOptions>,
    Extension(user_o): Extension<Option<User>>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    let user_id_o = user_o.as_ref().map(|u| u.id);

    let record = conditional_query_as!(
        BookableSchema,
        r#"SELECT
            bs.id,
            bs.label,
            bs.schedule::text[] as "schedule!",
            bs.asset_type_id,
            bs.slot_price,
            EXTRACT(EPOCH FROM bs.activation_grace_period)::bigint as activation_grace_period,
            bs.expiry_refund_percent
        FROM
            bookable_schemas bs
        WHERE account_bookable_perm_exists({user_id_o}, bs.asset_type_id, 'view'::bookable_perm) AND
        {#search}
        LIMIT {limit} OFFSET {offset}"#,
        #search = match &opts.search {
            Some(s) => "label ILIKE '%' || {s} || '%'",
            None => "TRUE",
        }
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/bookables/schemas/{id}", responses((status = 200, description = "OK", body=BookableSchema)))]
pub async fn get(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user_o): Extension<Option<User>>,
) -> Result<impl IntoResponse, VialoError> {
    let user_id_o = user_o.as_ref().map(|u| u.id);

    let record = sqlx::query_as!(
        BookableSchema,
        r#"SELECT
            bs.id,
            bs.label,
            bs.schedule::text[] as "schedule!",
            bs.asset_type_id,
            bs.slot_price,
            EXTRACT(EPOCH FROM bs.activation_grace_period)::bigint as activation_grace_period,
            bs.expiry_refund_percent
        FROM
            bookable_schemas bs
        WHERE bs.id = $1
          AND account_bookable_perm_exists($2, bs.asset_type_id, 'view'::bookable_perm)"#,
        id,
        user_id_o
    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(delete, path = "/bookables/schemas/{id}", responses((status = 204, description = "Deleted")))] //no body
pub async fn delete(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm_by_schema(user.id, id, BookablePerm::Admin, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let _record = query!(
        r#"DELETE FROM
            bookable_schemas
        WHERE
            id = $1"#,
        id
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[serde_as]
#[derive(Serialize, Deserialize, ToSchema)]
pub struct BookableSchemaPostOrPut {
    pub label: Option<String>,
    pub schedule: Vec<String>,
    pub asset_type_id: i32,
    pub slot_price: Option<i32>,
    #[serde_as(as = "Option<DurationSeconds<i64>>")]
    #[schema(value_type = Option<i64>)]
    pub activation_grace_period: Option<TimeDelta>,
    pub expiry_refund_percent: Option<i16>,
}

#[utoipa::path(post, path = "/bookables/schemas", request_body = BookableSchemaPostOrPut, responses((status = 201, description = "Created", body=BoardPostIdModel)))]
pub async fn post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookableSchemaPostOrPut>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm(
        Some(user.id),
        body.asset_type_id,
        BookablePerm::Admin,
        &data.db,
    )
    .await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let record = sqlx::query_as!(
        BoardPostIdModel,
        "INSERT INTO bookable_schemas (label, schedule, asset_type_id, slot_price, activation_grace_period, expiry_refund_percent) VALUES ($1, $2::time[], $3, $4, $5, $6) RETURNING id",
        body.label, body.schedule as Vec<String>, body.asset_type_id, body.slot_price, body.activation_grace_period.map(PgInterval::try_from)
          .transpose().map_err(|e| VialoError::AppError(StatusCode::BAD_REQUEST, e.to_string()))?,
        body.expiry_refund_percent
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(put, path = "/bookables/schemas/{id}", request_body=BookableSchemaPostOrPut, responses((status = 200, description = "Updated", body=BookableSchema)))]
pub async fn put(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookableSchemaPostOrPut>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm_by_schema(user.id, id, BookablePerm::Admin, &data.db).await?;
    require_asset_type_perm(
        Some(user.id),
        body.asset_type_id,
        BookablePerm::Admin,
        &data.db,
    )
    .await?;

    let existing_appointments = sqlx::query_scalar!(
        r#"SELECT EXISTS (
            (SELECT 1 FROM bookable_appointments ba
            JOIN LATERAL (
              SELECT DISTINCT ON (asset_id) schema_id, begins
              FROM bookable_schema_assignments
              WHERE asset_id = ba.asset_id
                AND begins <= lower(ba.during)
              ORDER BY asset_id, begins DESC
            ) bsa ON true
            WHERE schema_id = $1)
        ) AS "existing: bool""#,
        id
    )
    .fetch_one(&data.db)
    .await?
    .unwrap_or(false);

    if existing_appointments {
        return Err(VialoError::AppError(
            StatusCode::BAD_REQUEST,
            "existing_appointments".to_string(),
        ));
    }

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let record = sqlx::query_as!(
        BookableSchema,
        r#"UPDATE bookable_schemas SET (label, schedule, asset_type_id, slot_price, activation_grace_period, expiry_refund_percent) = ($1, $2::time[], $3, $4, $5, $6) WHERE id = $7
        RETURNING
        id,
        label,
        schedule::text[] as "schedule!",
        asset_type_id,
        slot_price,
        EXTRACT(EPOCH FROM activation_grace_period)::bigint as activation_grace_period,
        expiry_refund_percent"#,
        body.label, body.schedule as Vec<String>, body.asset_type_id, body.slot_price, body.activation_grace_period.map(PgInterval::try_from)
          .transpose().map_err(|e| VialoError::AppError(StatusCode::BAD_REQUEST, e.to_string()))?, body.expiry_refund_percent, id
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}
