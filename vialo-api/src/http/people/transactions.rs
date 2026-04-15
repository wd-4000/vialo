use super::{
    models::{
        PersonalTransactionModel, PersonalTransactionModelWithProductDetails, ProductType,
        TransactionStatus,
    },
    schemas::TransactionFilterOptions,
};

use crate::http::util::models::IdOrMeOrAllQuery;
use crate::{
    AppState,
    http::util::{JsonE, MaybeJsonE, User, VialoError, grab_authd_conn_user, grab_trans},
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::query_as;
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use uuid::Uuid;

pub async fn list(
    Query(opts): Query<TransactionFilterOptions>,
    Extension(User { id: user_id }): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);
    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    let record = conditional_query_as!(PersonalTransactionModel,
        r#"SELECT cl.id,
        CASE WHEN from_account IS NOT NULL THEN jsonb_build_object('id', from_account, 'full_name', ac_from.full_name) ELSE NULL END AS from_account,
        CASE WHEN to_account IS NOT NULL THEN jsonb_build_object('id', to_account, 'full_name', ac_to.full_name) ELSE NULL END AS to_account,
         credits, cl.created_at, cl.last_updated, cl.status as "status: TransactionStatus", cl.product as "product: ProductType"
        FROM credit_ledger cl
        LEFT JOIN accounts_people ac_from ON from_account = ac_from.id
        LEFT JOIN accounts_people ac_to ON to_account = ac_to.id
       {#account_where}
        ORDER BY created_at DESC
        LIMIT {limit} OFFSET {offset}"#, #account_where = match (opts.account_id) {
            IdOrMeOrAllQuery::Id(acid) => "WHERE to_account = {acid} OR from_account = {acid}",
            IdOrMeOrAllQuery::Me => "WHERE to_account = {user_id} OR from_account = {user_id}",
            IdOrMeOrAllQuery::All => ""
        })
        .fetch_all(&data.db)
    .await?;
    return Ok((
        StatusCode::OK,
        Json(json!({"status": "success","data": record})),
    ));
}

pub async fn get(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let record = query_as!(PersonalTransactionModelWithProductDetails,
        r#"SELECT cl.id,
        CASE WHEN from_account IS NOT NULL THEN jsonb_build_object('id', from_account, 'full_name', ac_from.full_name) ELSE NULL END AS from_account,
        CASE WHEN to_account IS NOT NULL THEN jsonb_build_object('id', to_account, 'full_name', ac_to.full_name) ELSE NULL END AS to_account,
         cl.credits, cl.label, cl.created_at, cl.last_updated, cl.status as "status: TransactionStatus", jsonb_build_object('type', cl.product, 'id', ba.id) AS product
        FROM credit_ledger cl
        LEFT JOIN accounts_people ac_from ON from_account = ac_from.id
        LEFT JOIN accounts_people ac_to ON to_account = ac_to.id
        LEFT JOIN bookable_appointments ba ON ba.transaction_id = cl.id
        WHERE cl.id = $1"#,id)
        .fetch_one(&data.db)
    .await?;
    return Ok((
        StatusCode::OK,
        Json(json!({"status": "success","data": record})),
    ));
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CreateTransactionSchema {
    pub from_account: Option<Uuid>,
    pub to_account: Option<Uuid>,
    pub label: Option<String>,
    pub credits: i32,
}

pub async fn post(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreateTransactionSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let id = sqlx::query_scalar!(
        "INSERT INTO credit_ledger (label, from_account, to_account, credits) VALUES ($1, $2, $3, $4) RETURNING id",
        body.label,
        body.from_account,
        body.to_account,
        body.credits,

    )
    .fetch_one(&mut *conn)
    .await?;

    return Ok((
        StatusCode::CREATED,
        Json(json!({"status": "ok", "data":{"id": id}})),
    ));
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UndoTransactionSchema {
    pub cancel_associated_product: Option<bool>,
}

pub async fn undo(
    State(data): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    MaybeJsonE(body): MaybeJsonE<UndoTransactionSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    // Check for an associated product. If there is one, figure out what to do
    if let Some(product_id) = sqlx::query_scalar!(
        "select id from bookable_appointments where transaction_id = $1 LIMIT 1",
        id
    )
    .fetch_optional(&mut *trans)
    .await?
    {
        match body.and_then(|b| b.cancel_associated_product) {
            Some(true) => {
                sqlx::query!(
                    "DELETE FROM bookable_appointments WHERE id = $1",
                    product_id
                )
                .execute(&mut *trans)
                .await?;
            }
            Some(false) => {}
            None => {
                return Err(VialoError::AppError(
                    StatusCode::BAD_REQUEST,
                    "has_associated_product".into(),
                ));
            }
        }
    }

    sqlx::query_scalar!(
        "UPDATE credit_ledger SET status = 'refunded', last_updated = NOW() WHERE id = $1 RETURNING id",
        id
    )
    .fetch_one(&mut *trans)
    .await?;

    trans.commit().await?;

    return Ok(StatusCode::NO_CONTENT);
}
