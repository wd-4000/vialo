use crate::{
    AppState, helpers,
    http::util::{JsonE, User, VialoError, grab_authd_conn_user, grab_trans},
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::prelude::FromRow;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Serialize, Debug, FromRow)]
pub struct IdentityModel {
    pub id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub full_name: Option<String>,
    pub email: Option<String>,
}

#[derive(Serialize, Debug, FromRow)]
pub struct PersonCandidateModel {
    pub id: Uuid,
    pub full_name: Option<String>,
    pub email: Option<String>,
    pub room_name: Option<String>,
}

#[derive(Serialize)]
struct IdentityResponse {
    #[serde(flatten)]
    identity: IdentityModel,
    candidates: Vec<PersonCandidateModel>,
}

#[utoipa::path(get, path = "/people/identities/{id}", responses((status = 200, description = "OK")))]
pub async fn get(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let person = sqlx::query_as!(IdentityModel, r#"SELECT i.id, ap.id as "account_id: Option<Uuid>", i.full_name, i.email FROM identities i LEFT JOIN accounts_people ap ON ap.auth_id = i.id WHERE i.id = $1"#, id)
        .fetch_one(&data.db)
        .await?;

    // Search for people with a roughly similar name if no account linked
    let candidates: Vec<PersonCandidateModel> = if person.account_id.is_none() {
        if let Some(ref full_name) = person.full_name {
            sqlx::query_as!(PersonCandidateModel, r#"SELECT a.id, a.full_name, a.email::text as "email?: String", r.label as "room_name" FROM accounts_people a LEFT JOIN res_leases l ON a.id = l.tenant_id
            LEFT JOIN res_rooms r ON l.room_id = r.id WHERE a.auth_id IS NULL AND a.full_name IS NOT NULL AND string_to_array(LOWER(a.full_name), ' ') @> string_to_array(LOWER($1), ' ') AND string_to_array(LOWER($1), ' ') @> string_to_array(LOWER(a.full_name), ' ')"#, full_name)
                .fetch_all(&data.db)
                .await?
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    Ok(Json(IdentityResponse {
        identity: person,
        candidates,
    }))
}

#[utoipa::path(delete, path = "/people/identities/{id}", responses((status = 204, description = "Deleted")))]
pub async fn delete(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    helpers::people::delete_identity(id, &mut trans)
        .await
        .map_err(|e| VialoError::Anyhow(e))?;

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MergeUserSchema {
    pub account_id: Uuid,
}

#[utoipa::path(post, path = "/people/identities/{id}/merge", responses((status = 200, description = "Merged")))]
pub async fn merge(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<MergeUserSchema>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    if let Some(account_id) = sqlx::query_scalar!(
        r#"UPDATE accounts_people SET auth_id = $1 WHERE id = $2 AND auth_id IS NULL RETURNING id"#,
        id,
        body.account_id,
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        return Ok(Json(json!({"account_id": account_id})));
    }

    let person = sqlx::query!(
        r#"SELECT auth_id, id as account_id FROM accounts_people WHERE id = $1"#,
        body.account_id,
    )
    .fetch_one(&mut *conn)
    .await?;

    Err(VialoError::AppErrorWithDetails(
        StatusCode::CONFLICT,
        "identity_already_bound".into(),
        json!({"account_id": person.account_id, "auth_id": person.auth_id}),
    ))
}
