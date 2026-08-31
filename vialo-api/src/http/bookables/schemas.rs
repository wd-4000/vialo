use super::models::BoardPostIdModel;
use super::permissions::{
    BookablePerm, require_asset_type_perm, require_asset_type_perm_by_schema,
};
use crate::AppState;
use crate::http::util::grab_authd_conn_user;
use crate::http::util::{JsonE, User, VialoError, clamp_pagination};
use axum::extract::Path;
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use serde_with::{DurationSeconds, serde_as};
use sqlx::PgConnection;
use sqlx::postgres::types::PgInterval;
use sqlx::query;
use sqlx_conditional_queries::conditional_query_as;
use std::i64;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

#[derive(Serialize, Deserialize, ToSchema)]
pub struct BookableSchema {
    pub id: i32,
    pub label: Option<String>,
    pub schedule: Vec<String>,
    pub asset_type_id: i32,
    pub slot_price: Option<i32>,
    pub requires_activation: bool,
    pub activation_grace_period: Option<i64>,
    pub expiry_refund_percent: Option<i16>,
}

/// Schema body minus asset_type_id, for schemas created inline with their asset
#[serde_as]
#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct NewSchemaInline {
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_255")]
    pub label: Option<String>,
    pub schedule: Vec<String>,
    pub slot_price: Option<i32>,
    #[serde_as(as = "Option<DurationSeconds<i64>>")]
    #[schema(value_type = Option<i64>)]
    pub activation_grace_period: Option<TimeDelta>,
    pub expiry_refund_percent: Option<i16>,
    pub requires_activation: bool,
}

fn validate_schema_fields(body: &NewSchemaInline) -> Result<(), VialoError> {
    if body.slot_price.is_some_and(|v| v < 0) {
        return Err(VialoError::AppError(
            StatusCode::BAD_REQUEST,
            "slot_price must be >= 0".into(),
        ));
    }
    if body.requires_activation {
        if body
            .expiry_refund_percent
            .is_some_and(|v| !(0..=100).contains(&v))
        {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "expiry_refund_percent must be between 0 and 100".into(),
            ));
        }
    } else {
        if body.activation_grace_period.is_some() || body.expiry_refund_percent.is_some() {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "activation_grace_period or expiry_refund_percent must not be set when requires_activation is false".into(),
            ));
        }
    }

    Ok(())
}

/// Scoped by asset_type_id so a picker can offer only schemas the asset can use.
#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct SchemaListOptions {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    pub asset_type_id: Option<i32>,
}

#[utoipa::path(get, path = "/bookables/schemas", params(SchemaListOptions), responses((status = 200, description = "OK", body=Vec<BookableSchema>)))]
pub async fn list(
    Query(opts): Query<SchemaListOptions>,
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
            bs.expiry_refund_percent,
            bs.requires_activation
        FROM
            bookable_schemas bs
        WHERE account_bookable_perm_exists({user_id_o}, bs.asset_type_id, 'view'::bookable_perm) AND
        {#asset_type} AND
        {#search}
        LIMIT {limit} OFFSET {offset}"#,
        #asset_type = match &opts.asset_type_id {
            Some(t) => "bs.asset_type_id = {t}",
            None => "TRUE",
        },
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
            bs.expiry_refund_percent,
            bs.requires_activation
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
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_255")]
    pub label: Option<String>,
    pub schedule: Vec<String>,
    pub asset_type_id: i32,
    pub slot_price: Option<i32>,
    #[serde_as(as = "Option<DurationSeconds<i64>>")]
    #[schema(value_type = Option<i64>)]
    pub activation_grace_period: Option<TimeDelta>,
    pub expiry_refund_percent: Option<i16>,
    pub requires_activation: bool,
}

impl BookableSchemaPostOrPut {
    fn split(self) -> (i32, NewSchemaInline) {
        (
            self.asset_type_id,
            NewSchemaInline {
                label: self.label,
                schedule: self.schedule,
                slot_price: self.slot_price,
                activation_grace_period: self.activation_grace_period,
                expiry_refund_percent: self.expiry_refund_percent,
                requires_activation: self.requires_activation,
            },
        )
    }
}

/// Validates and inserts a schema, returning its id. The caller checks permissions.
pub async fn insert_schema(
    db: &mut PgConnection,
    body: NewSchemaInline,
    asset_type_id: i32,
) -> Result<i32, VialoError> {
    validate_schema_fields(&body)?;

    let id = sqlx::query_scalar!(
        "INSERT INTO bookable_schemas (label, schedule, asset_type_id, slot_price, activation_grace_period, expiry_refund_percent, requires_activation) VALUES ($1, $2::time[], $3, $4, $5, $6, $7) RETURNING id",
        body.label, body.schedule as Vec<String>, asset_type_id, body.slot_price, body.activation_grace_period.map(PgInterval::try_from)
          .transpose().map_err(|e| VialoError::AppError(StatusCode::BAD_REQUEST, e.to_string()))?,
        body.expiry_refund_percent,
        body.requires_activation,
    )
    .fetch_one(&mut *db)
    .await?;

    Ok(id)
}

#[utoipa::path(post, path = "/bookables/schemas", request_body = BookableSchemaPostOrPut, responses((status = 201, description = "Created", body=BoardPostIdModel)))]
pub async fn post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookableSchemaPostOrPut>,
) -> Result<impl IntoResponse, VialoError> {
    let (asset_type_id, schema) = body.split();
    require_asset_type_perm(Some(user.id), asset_type_id, BookablePerm::Admin, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let id = insert_schema(&mut conn, schema, asset_type_id).await?;

    Ok((StatusCode::CREATED, Json(BoardPostIdModel { id })))
}

#[utoipa::path(put, path = "/bookables/schemas/{id}", request_body=BookableSchemaPostOrPut, responses((status = 200, description = "Updated", body=BookableSchema)))]
pub async fn put(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<BookableSchemaPostOrPut>,
) -> Result<impl IntoResponse, VialoError> {
    let (asset_type_id, schema) = body.split();
    validate_schema_fields(&schema)?;
    require_asset_type_perm_by_schema(user.id, id, BookablePerm::Admin, &data.db).await?;
    require_asset_type_perm(Some(user.id), asset_type_id, BookablePerm::Admin, &data.db).await?;

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
        r#"UPDATE bookable_schemas SET (label, schedule, asset_type_id, slot_price, activation_grace_period, expiry_refund_percent, requires_activation) = ($1, $2::time[], $3, $4, $5, $6, $7) WHERE id = $8
        RETURNING
        id,
        label,
        schedule::text[] as "schedule!",
        asset_type_id,
        slot_price,
        EXTRACT(EPOCH FROM activation_grace_period)::bigint as activation_grace_period,
        expiry_refund_percent,
        requires_activation
        "#,
        schema.label, schema.schedule as Vec<String>, asset_type_id, schema.slot_price, schema.activation_grace_period.map(PgInterval::try_from)
          .transpose().map_err(|e| VialoError::AppError(StatusCode::BAD_REQUEST, e.to_string()))?, schema.expiry_refund_percent, schema.requires_activation, id
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}
