use crate::{
    AppState,
    helpers::{
        encryption::{self, Encrypted},
        people::generate_amenities_login,
    },
    http::util::{User, VialoError, grab_authd_conn_user, grab_trans},
    permissions::{AppRole, check_app_role},
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use sqlx::prelude::FromRow;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

/// A person's amenities login (printer, kiosk).
#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct AmenitiesLoginModel {
    pub username: String,
    /// Reversibly encrypted at rest, the printer and kiosk hold it in the clear.
    #[serde(serialize_with = "encryption::serialize_exposed")]
    #[schema(value_type = String)]
    pub password: Encrypted<String>,
}

async fn get_amenities_impl(
    id: Uuid,
    data: Arc<AppState>,
) -> Result<impl IntoResponse, VialoError> {
    // permission check in the caller!

    let login = sqlx::query_as!(
        AmenitiesLoginModel,
        r#"SELECT amenities_username AS "username!", amenities_pin AS "password!: Encrypted<String>"
        FROM accounts_people
        WHERE id = $1 AND amenities_username IS NOT NULL AND amenities_pin IS NOT NULL"#,
        id
    )
    .fetch_optional(&data.db)
    .await?
    .ok_or(VialoError::NotFound())?;

    Ok(Json(login))
}

#[utoipa::path(get, path = "/people/{id}/amenities", responses((status = 200, description = "OK", body=AmenitiesLoginModel)))]
pub async fn get_by_id(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    if user.id != id {
        check_app_role(user, AppRole::AccountManager, &data.db).await?;
    }
    get_amenities_impl(id, data).await
}

#[utoipa::path(get, path = "/people/me/amenities", responses((status = 200, description = "OK", body=AmenitiesLoginModel)))]
pub async fn get_me(
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    get_amenities_impl(user.id, data).await
}

#[utoipa::path(post, path = "/people/{id}/amenities/generate", responses((status = 200, description = "OK", body=AmenitiesLoginModel)))]
pub async fn generate(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM accounts_people WHERE id = $1) AS "exists!""#,
        id
    )
    .fetch_optional(&mut *trans)
    .await?
    .unwrap_or(false);

    if !exists {
        return Err(VialoError::NotFound());
    }

    // The update fires the printer sync trigger.
    let login = generate_amenities_login(id, &mut trans).await?;

    trans.commit().await?;

    // PINs leave the API only here and in the GET.
    Ok(Json(AmenitiesLoginModel {
        username: login.username,
        password: Encrypted::new(&login.pin),
    }))
}

#[utoipa::path(delete, path = "/people/{id}/amenities", responses((status = 204, description = "Deleted")))]
pub async fn delete(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let result = sqlx::query!(
        "UPDATE accounts_people SET (amenities_username, amenities_pin) = (NULL, NULL) WHERE id = $1",
        id
    )
    .execute(&mut *trans)
    .await?;

    if result.rows_affected() == 0 {
        return Err(VialoError::NotFound());
    }

    // Drop the printer mirror too. The row delete fires the device account
    // deletion trigger.
    sqlx::query!("DELETE FROM subsystem_printer_context WHERE id = $1", id)
        .execute(&mut *trans)
        .await?;

    trans.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
