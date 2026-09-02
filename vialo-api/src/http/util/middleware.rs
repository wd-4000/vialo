use crate::{
    config::AuthConfig,
    health::add_health_event,
    helpers::grab_authd_conn_subsystem,
    http::{
        history::models::Subsystem,
        util::{AuthError, UserSuspendedReason, VialoError, grab_trans},
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
use axum_extra::extract::CookieJar;
use ory_kratos_client::apis::frontend_api::to_session;
use reqwest::StatusCode;
use serde_json::json;
use std::sync::Arc;

pub async fn auth_required(request: Request, next: Next) -> Result<Response, VialoError> {
    if request.extensions().get::<User>().is_none() {
        match request.extensions().get::<AuthError>() {
            Some(AuthError::Suspended(user_suspended)) => {
                return Err(VialoError::AppErrorWithDetails(
                    StatusCode::FORBIDDEN,
                    "suspended".into(),
                    user_suspended.as_ref().into(),
                ));
            }
            Some(other) => {
                return Err(VialoError::AppError(
                    StatusCode::UNAUTHORIZED,
                    other.as_ref().into(),
                ));
            }
            None => {}
        }
        return Err(VialoError::AppError(
            StatusCode::UNAUTHORIZED,
            "unauthorized".into(),
        ));
    }

    Ok(next.run(request).await)
}

pub async fn auth_middleware(
    State(app_state): State<Arc<AppState>>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Result<Response, VialoError> {
    if let AuthConfig::Mock { uuid, .. } = &app_state.config.auth {
        let user = User {
            id: *uuid,
            ..Default::default()
        };
        request.extensions_mut().insert(Some(user.clone()));
        request.extensions_mut().insert(user);
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

        if x_session_token.is_none() && jar.get("ory_kratos_session").is_none() {
            request.extensions_mut().insert(AuthError::Unauthorized);

            return Ok(next.run(request).await);
        }
        // Call Kratos to validate session
        match to_session(
            kratos_frontend,
            x_session_token.as_deref(),
            cookie.as_deref(),
            None,
        )
        .await
        {
            Ok(session) => {
                if let Some(identity) = session.identity {
                    if let Ok(auth_id) = uuid::Uuid::parse_str(&identity.id) {
                        if let Some(user_record) = sqlx::query!(
                            "SELECT id, manually_suspended, membership_end < NOW() as expired from accounts_people WHERE auth_id = $1",
                            auth_id
                        )
                        .fetch_optional(&app_state.db)
                        .await?
                        {
                            if user_record.manually_suspended {
                                request
                                    .extensions_mut()
                                    .insert(AuthError::Suspended(UserSuspendedReason::ManuallySuspended));
                            }

                            if user_record.expired.is_some_and(|x| x) {
                                request
                                    .extensions_mut()
                                    .insert(AuthError::Suspended(UserSuspendedReason::Expired));
                            }

                            // All good
                            let user = User {
                                id: user_record.id,
                                ..Default::default()
                            };
                            request.extensions_mut().insert(Some(user.clone()));
                            request.extensions_mut().insert(user);
                            return Ok(next.run(request).await);
                        }

                        // Identity not present
                        if !sqlx::query_scalar!(
                            r#"SELECT EXISTS (SELECT 1 from identities WHERE id = $1) AS "exists!""#,
                            auth_id
                        )
                        .fetch_one(&app_state.db)
                        .await?
                        {
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
                                None,
                            )
                            .await;

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
                            let transformed_identity = crate::helpers::people::IdentityModel {
                                id: auth_id,
                                email: trait_str("email"),
                                full_name: trait_str("full_name"),
                                phone: trait_str("phone"),
                                room: trait_str("room"),
                                account_id: None,
                            };
                            let mut trans = grab_trans(&mut conn).await?;
                            helpers::people::update_identity(transformed_identity, &mut trans)
                                .await
                                .map_err(VialoError::Anyhow)?;
                            trans.commit().await?;
                        }

                        request
                            .extensions_mut()
                            .insert(AuthError::Suspended(UserSuspendedReason::NotVerified));
                    } else {
                        tracing::warn!("Couldn't parse Kratos identity ID: {}", identity.id);
                    }
                }
            }
            Err(e) => {
                tracing::debug!("Invalid Kratos session: {:?}", e);
                request.extensions_mut().insert(AuthError::InvalidSession);
            }
        }
    }

    request.extensions_mut().insert(Option::<User>::None);
    Ok(next.run(request).await)
}
