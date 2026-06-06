use crate::{
    AppState,
    http::util::{JsonE, User, VialoError, grab_authd_conn_user},
    permissions::{AppRole, check_app_role},
};
use std::sync::Arc;

use super::models::NmsType;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct NmsLinkModel {
    pub nms_connector_id: i32,
    #[serde(rename = "type")]
    pub r#type: NmsType,
    pub hostname: String,
    pub nms_id: String,
}

#[derive(Deserialize, ToSchema)]
pub struct PutNmsLinkSchema {
    pub nms_id: String,
}

#[utoipa::path(get, path = "/network/networks/{id}/nms-links", responses((status = 200, description = "OK", body = Vec<NmsLinkModel>)))]
pub async fn list(
    Path(id): Path<i32>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user, AppRole::NetworkManager, &data.db).await?;

    let records = sqlx::query_as!(
        NmsLinkModel,
        r#"SELECT nnl.nms_connector_id, nc.type as "type: NmsType", nc.hostname, nnl.nms_id
           FROM net_network_nms_link nnl
           JOIN net_nms_connectors nc ON nc.id = nnl.nms_connector_id
           WHERE nnl.network_id = $1"#,
        id
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(records)))
}

#[utoipa::path(put, path = "/network/networks/{network_id}/nms-links/{connector_id}", request_body = PutNmsLinkSchema, responses((status = 204, description = "Upserted")))]
pub async fn put(
    Path((network_id, connector_id)): Path<(i32, i32)>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<PutNmsLinkSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        "INSERT INTO net_network_nms_link (network_id, nms_connector_id, nms_id)
         VALUES ($1, $2, $3)
         ON CONFLICT (network_id, nms_connector_id) DO UPDATE SET nms_id = $3",
        network_id,
        connector_id,
        body.nms_id,
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/network/networks/{network_id}/nms-links/{connector_id}", responses((status = 204, description = "Deleted")))]
pub async fn delete(
    Path((network_id, connector_id)): Path<(i32, i32)>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::NetworkManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let rows_affected = sqlx::query!(
        "DELETE FROM net_network_nms_link WHERE network_id = $1 AND nms_connector_id = $2",
        network_id,
        connector_id,
    )
    .execute(&mut *conn)
    .await?
    .rows_affected();

    if rows_affected == 0 {
        return Err(VialoError::NotFound());
    }

    Ok(StatusCode::NO_CONTENT)
}
