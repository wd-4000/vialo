use super::{
    models::{
        LedgerEntryType, PersonalTransactionModel, PersonalTransactionModelWithProductDetails,
        TransactionStatus,
    },
    schemas::TransactionFilterOptions,
};

use crate::{
    AppState,
    http::util::{
        JsonE, MaybeJsonE, User, VialoError, clamp_pagination, grab_authd_conn_user, grab_trans,
        models::{AccountEmbed, IdOrAllQuery, ProductEmbed, ProductType},
    },
    permissions::{AppRole, check_app_role},
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

#[utoipa::path(get, path = "/people/transactions", params(TransactionFilterOptions), responses((status = 200, description = "OK", body=Vec<PersonalTransactionModel>)))]
pub async fn list(
    Query(opts): Query<TransactionFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    // Permission check in case we receive an account ID different from the current account
    let resolved_account_id = opts.account_id.resolve(user.id);
    if resolved_account_id != IdOrAllQuery::Id(user.id) {
        check_app_role(user.clone(), AppRole::CreditManager, &data.db).await?;
    }

    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    let record = conditional_query_as!(PersonalTransactionModel,
        r#"SELECT cl.id,
        CASE WHEN from_account IS NOT NULL THEN jsonb_build_object('id', from_account, 'full_name', ac_from.full_name, 'type', 'person') ELSE NULL END AS "from_account: AccountEmbed",
        CASE WHEN to_account IS NOT NULL THEN jsonb_build_object('id', to_account, 'full_name', ac_to.full_name, 'type', 'person') ELSE NULL END AS "to_account: AccountEmbed",
         credits, cl.created_at, cl.last_updated, cl.status as "status: TransactionStatus", cl.refund_of, cl.product as "product: ProductType", cl.label, cl.entry_type as "entry_type: LedgerEntryType"
        FROM credit_ledger cl
        LEFT JOIN accounts_people ac_from ON from_account = ac_from.id
        LEFT JOIN accounts_people ac_to ON to_account = ac_to.id
       {#account_where}
        ORDER BY created_at DESC
        LIMIT {limit} OFFSET {offset}"#, #account_where = match (resolved_account_id) {
            IdOrAllQuery::Id(acid) => "WHERE to_account = {acid} OR from_account = {acid}",
            IdOrAllQuery::All => ""
        })
        .fetch_all(&data.db)
    .await?;
    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/people/transactions/{id}", responses((status = 200, description = "OK", body=PersonalTransactionModelWithProductDetails)))]
pub async fn get(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    let is_credit_manager = check_app_role(user.clone(), AppRole::CreditManager, &data.db)
        .await
        .is_ok();
    let user_id = user.id;
    let record = conditional_query_as!(PersonalTransactionModelWithProductDetails,
        r#"SELECT cl.id,
        CASE WHEN from_account IS NOT NULL THEN jsonb_build_object('id', from_account, 'full_name', ac_from.full_name, 'type', 'person') ELSE NULL END AS "from_account: AccountEmbed",
        CASE WHEN to_account IS NOT NULL THEN jsonb_build_object('id', to_account, 'full_name', ac_to.full_name, 'type', 'person') ELSE NULL END AS "to_account: AccountEmbed",
         cl.credits, cl.label, cl.created_at, cl.last_updated, cl.status as "status: TransactionStatus", cl.refund_of, CASE WHEN ba.id IS NOT NULL THEN jsonb_build_object('type', cl.product, 'id', ba.id) ELSE NULL END AS "product: ProductEmbed", cl.entry_type as "entry_type: LedgerEntryType"
        FROM credit_ledger cl
        LEFT JOIN accounts_people ac_from ON from_account = ac_from.id
        LEFT JOIN accounts_people ac_to ON to_account = ac_to.id
        LEFT JOIN bookable_appointments ba ON ba.transaction_id = cl.id
        WHERE cl.id = {id} {#WH_MANAGER}"#,
        #WH_MANAGER = match(is_credit_manager) {
            false => "AND (cl.from_account = {user_id} OR cl.to_account = {user_id})",
            _ => "",
        })
        .fetch_one(&data.db)
    .await?;
    Ok((StatusCode::OK, Json(record)))
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreateTransactionSchema {
    pub from_account: Option<Uuid>,
    pub to_account: Option<Uuid>,
    pub label: Option<String>,
    pub credits: i32,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedTransactionResponse {
    pub id: Uuid,
}

#[utoipa::path(post, path = "/people/transactions", request_body=CreateTransactionSchema, responses((status = 201, description = "Created", body=CreatedTransactionResponse)))]
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

    Ok((StatusCode::CREATED, Json(CreatedTransactionResponse { id })))
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct UndoTransactionSchema {
    pub cancel_associated_product: Option<bool>,
}

#[utoipa::path(post, path = "/people/transactions/{id}/undo", request_body=UndoTransactionSchema, responses((status = 204, description = "Undone")))]
pub async fn undo(
    State(data): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    MaybeJsonE(body): MaybeJsonE<UndoTransactionSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    // Check for an associated product. If there is one, figure out what to do
    let product = sqlx::query!(
        "SELECT id, asset_id FROM bookable_appointments WHERE transaction_id = $1 LIMIT 1",
        id
    )
    .fetch_optional(&mut *trans)
    .await?;

    let mut cancelled_asset_id: Option<i32> = None;
    if let Some(product) = product {
        match body.and_then(|b| b.cancel_associated_product) {
            Some(true) => {
                sqlx::query!(
                    "UPDATE bookable_appointments SET cancelled_at = now(), cancellation_reason = 'admin' WHERE id = $1",
                    product.id
                )
                .execute(&mut *trans)
                .await?;
                cancelled_asset_id = Some(product.asset_id);
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

    sqlx::query!(
        "INSERT INTO credit_ledger (to_account, credits, refund_of)
         SELECT from_account, credits, id FROM credit_ledger WHERE id = $1",
        id
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;

    if let Some(asset_id) = cancelled_asset_id {
        let evil_data = data.clone();
        tokio::spawn(async move {
            crate::bookables::fetch_and_broadcast_status(&evil_data, [asset_id]).await;
        });
    }

    Ok(StatusCode::NO_CONTENT)
}
