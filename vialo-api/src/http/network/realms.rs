use crate::{
    AppState,
    http::util::{JsonE, User, VialoError, grab_authd_conn_user},
    permissions::{AppRole, check_app_role},
};
use std::sync::Arc;

use super::{
    models::RealmModel,
    schemas::{RealmFilterOptions, SetDefaultRealmSchema, UpdateRealmSchema},
};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use sqlx::query_as;
use sqlx_conditional_queries::conditional_query_as;
use uuid::Uuid;

#[utoipa::path(get, path = "/network/realms", params(RealmFilterOptions), responses((status = 200, description = "OK", body=Vec<RealmModel>)))]
pub async fn list_realms(
    Query(opts): Query<RealmFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user, AppRole::NetworkManager, &data.db).await?;
    let limit = opts.limit.unwrap_or(10);
    let offset = (opts.page.unwrap_or(1) - 1) * limit;
    let records =  conditional_query_as!(
        RealmModel,
        "SELECT nr.* FROM net_realms nr {#account} {#search} ORDER BY id ASC LIMIT {limit} OFFSET {offset}",
        #account = match opts.account_id{
            Some(account_id) => "JOIN net_realm_assignments nra ON nra.realm_id = nr.id WHERE nra.account_id = {account_id}",
                _ => "WHERE TRUE"
        },
        #search = match opts.search {
            Some(src) => "AND (ipv4_subnet::text ILIKE '%' || {src} || '%' OR ipv4_nat::text ILIKE '%' || {src} || '%' OR ipv6_prefix::text ILIKE '%' || {src} || '%')",
            _ => ""
        }

    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(records)))
}

#[utoipa::path(get, path = "/network/realms/{id}", responses((status = 200, description = "OK", body=RealmModel)))]
pub async fn get_realm(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    // NetworkManagers can fetch any realm; residents can only fetch their own assigned realm.
    if check_app_role(user.clone(), AppRole::NetworkManager, &data.db)
        .await
        .is_err()
    {
        let owns = sqlx::query_scalar!(
                r#"SELECT COUNT(*) > 0 AS "owns: bool" FROM net_realm_assignments WHERE account_id = $1 AND realm_id = $2"#,
                user.id,
                id,
            )
        .fetch_one(&data.db)
        .await
        .map_err(|e| VialoError::Anyhow(e.into()))?
        .unwrap_or(false);

        if !owns {
            return Err(VialoError::Forbidden());
        }
    }

    let records = query_as!(RealmModel, "SELECT * FROM net_realms WHERE id = $1", id)
        .fetch_one(&data.db)
        .await?;

    Ok((StatusCode::OK, Json(records)))
}

#[utoipa::path(put, path = "/network/realms/{id}", request_body = UpdateRealmSchema, responses((status = 204, description = "Updated")))] // no body
pub async fn put_realm(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<UpdateRealmSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let result = sqlx::query!(
        "UPDATE net_realms SET ipv4_subnet = coalesce($1, ipv4_subnet), ipv4_nat = coalesce($2, ipv4_nat), ipv6_prefix = coalesce($3, ipv6_prefix), vlan = coalesce($4, vlan) WHERE id = $5",
        body.ipv4_subnet, body.ipv4_nat, body.ipv6_prefix, body.vlan, id
    )
    .execute(&mut *conn)
    .await?;

    match result.rows_affected() {
        1 => Ok(StatusCode::NO_CONTENT),
        _ => Err(VialoError::NotFound()),
    }
}

#[utoipa::path(post, path = "/network/realms/set_default", request_body = SetDefaultRealmSchema, responses((status = 204, description = "Updated")))] // no body
pub async fn set_default_realm(
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<SetDefaultRealmSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    if let Some(realm_id) = body.realm_id {
        let result = sqlx::query!(
        "INSERT INTO net_realm_assignments (account_id, realm_id) VALUES ($1, $2) ON CONFLICT (account_id) DO UPDATE SET realm_id = $2",
        body.account_id, realm_id
    )
    .execute(&mut *conn)
    .await?;

        match result.rows_affected() {
            1 => Ok(StatusCode::NO_CONTENT),
            _ => Err(VialoError::NotFound()),
        }
    } else {
        sqlx::query!(
            "DELETE FROM net_realm_assignments WHERE account_id = $1",
            body.account_id
        )
        .execute(&mut *conn)
        .await?;

        Ok(StatusCode::NO_CONTENT)
    }
}
