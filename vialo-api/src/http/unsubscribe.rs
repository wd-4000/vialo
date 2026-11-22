//! Unsubscribe links from board emails. The token is the board_subscriptions id.
//! POST unsubscribes, which is also what mail clients send for one-click unsubscribe (RFC 8058).

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header::ACCEPT},
    response::{IntoResponse, Redirect, Response},
};
use serde::Serialize;
use sqlx::query_scalar;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    AppState,
    helpers::grab_authd_conn_subsystem,
    http::util::{VialoError, grab_authd_conn_user},
};

#[derive(Serialize, ToSchema)]
pub struct UnsubscribeInfo {
    /// Board the token unsubscribes from
    pub board: String,
}

/// Browsers go to user-ui's dialog, which fetches this as JSON
#[utoipa::path(get, path = "/unsubscribe/{token}", params(("token" = Uuid, Path)),
    responses(
        (status = 200, description = "With Accept: application/json", body = UnsubscribeInfo),
        (status = 303, description = "Otherwise, to user-ui's unsubscribe dialog"),
        (status = 404, description = "Unknown token, or already unsubscribed"),
    ))]
pub async fn info(
    State(data): State<Arc<AppState>>,
    Path(token): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, VialoError> {
    let wants_json = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"));
    if !wants_json {
        let page = &data.config.email.url.unsubscribe_page;
        return Ok(Redirect::to(&format!("{page}{token}")).into_response());
    }

    let board = query_scalar!(
        r#"SELECT get_i18n_string(b.label, ARRAY['de', 'en']) AS "label!"
        FROM board_subscriptions bs JOIN boards b ON b.id = bs.board_id
        WHERE bs.id = $1"#,
        token
    )
    .fetch_optional(&data.db)
    .await?
    .ok_or(VialoError::NotFound())?;
    Ok(Json(UnsubscribeInfo { board }).into_response())
}

#[utoipa::path(post, path = "/unsubscribe/{token}", params(("token" = Uuid, Path)),
    responses((status = 204, description = "Unsubscribed, or already was")))]
pub async fn unsubscribe(
    State(data): State<Arc<AppState>>,
    Path(token): Path<Uuid>,
) -> Result<StatusCode, VialoError> {
    let account_id = query_scalar!(
        "SELECT account_id FROM board_subscriptions WHERE id = $1",
        token
    )
    .fetch_optional(&data.db)
    .await?;

    if let Some(account_id) = account_id {
        let mut conn = match account_id {
            Some(id) => grab_authd_conn_user(&data.db, id).await?,
            None => grab_authd_conn_subsystem(&data.db, "app").await?,
        };
        sqlx::query!("DELETE FROM board_subscriptions WHERE id = $1", token)
            .execute(&mut *conn)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
