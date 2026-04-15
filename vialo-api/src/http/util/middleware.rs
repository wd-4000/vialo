use crate::{
    config::AuthConfig,
    health::add_health_event,
    helpers::grab_authd_conn_subsystem,
    http::{
        history::models::Subsystem,
        util::{VialoError, grab_trans},
    },
};

use super::super::AppState;
use super::User;
use crate::helpers;
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use ory_kratos_client::apis::frontend_api::to_session;
use reqwest::StatusCode;
use serde_json::json;
use std::sync::Arc;
pub async fn auth_required(
    // State(app_state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if request.extensions().get::<User>().is_none() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(request).await)
}

pub async fn auth_middleware(
    State(app_state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, VialoError> {
    if let AuthConfig::Mock { uuid, .. } = &app_state.config.auth {
        request.extensions_mut().insert(Some(User { id: *uuid }));
        request.extensions_mut().insert(User { id: *uuid });
        return Ok(next.run(request).await);
    } else if let Some(kratos_frontend) = app_state.kratos_config.as_ref().map(|c| &c.frontend) {
        // Extract session token from headers or cookies
        let x_session_token = request
            .headers()
            .get("X-Session-Token")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let cookie = request
            .headers()
            .get("Cookie")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Call Kratos to validate session
        match to_session(
            &kratos_frontend,
            x_session_token.as_deref(),
            cookie.as_deref(),
            None,
        )
        .await
        {
            Ok(session) => {
                if let Some(identity) = session.identity {
                    if let Ok(auth_id) = uuid::Uuid::parse_str(&identity.id) {
                        if let Some(account_id) = sqlx::query_scalar!(
                            "SELECT id from accounts_people WHERE auth_id = $1",
                            auth_id
                        )
                        .fetch_optional(&app_state.db)
                        .await?
                        {
                            request
                                .extensions_mut()
                                .insert(Some(User { id: account_id }));
                            request.extensions_mut().insert(User { id: account_id });
                            return Ok(next.run(request).await);
                        } else {
                            tracing::warn!(
                                "Kratos de-sync detected. Identity ID: {}. Fixing.",
                                identity.id
                            );
                            let mut conn = grab_authd_conn_subsystem(&app_state.db, "app").await?;
                            add_health_event(
                                &mut *conn,
                                Subsystem::App,
                                "identity_desync",
                                Some(json!({"id": identity.id})),
                                2,
                                false,
                            )
                            .await
                            .map_err(|e| VialoError::Anyhow(e.into()))?;

                            // Do the same thing as the jsonnet transform in the Kratos config
                            // This is a bit messy but frankly shouldn't happen in prod anyway

                            // helper to get identity trait
                            let trait_str = |key| {
                                identity
                                    .traits
                                    .as_ref()
                                    .and_then(|t| t.get(key))
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string)
                            };
                            let transformed_identity = crate::hooks::IdentityModel {
                                id: Some(auth_id),
                                email: trait_str("email"),
                                full_name: trait_str("full_name"),
                            };
                            let mut trans = grab_trans(&mut conn).await?;
                            helpers::people::update_identity(transformed_identity, &mut trans)
                                .await
                                .map_err(|e| VialoError::Anyhow(e.into()))?;
                            trans.commit().await?;
                        }
                    } else {
                        tracing::warn!("Couldn't parse Kratos identity ID: {}", identity.id);
                    }
                }
            }
            Err(e) => {
                tracing::debug!("Invalid Kratos session: {:?}", e);
            }
        }
    }

    request.extensions_mut().insert(Option::<User>::None);
    Ok(next.run(request).await)
}
