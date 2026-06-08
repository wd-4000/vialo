use crate::{
    AppState,
    helpers::encryption::{self, Encrypted},
    http::util::{JsonE, User, VialoError, clamp_pagination, grab_authd_conn_user, models::AccountEmbed},
    permissions::{AppRole, check_app_role},
};
use std::sync::Arc;

use super::{
    models::{CredentialModelWithAccountEmbed, CredentialModelWithPasswordAndAccountEmbed},
    schemas::NetworkFilterOptions,
};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde::Deserialize;
use sqlx::{query, query_as};
use sqlx_conditional_queries::conditional_query_as;
use utoipa::ToSchema;
use uuid::Uuid;

/// Returns Ok(()) if the user owns the credential OR holds NetworkManager.
async fn check_own_or_network_manager(
    user: &User,
    cred_id: Uuid,
    db: &sqlx::PgPool,
) -> Result<(), VialoError> {
    if check_app_role(user.clone(), AppRole::NetworkManager, db)
        .await
        .is_ok()
    {
        return Ok(());
    }

    let owns = sqlx::query_scalar!(
        r#"SELECT COUNT(*) > 0 AS "owns: bool" FROM net_cred WHERE id = $1 AND account_id = $2"#,
        cred_id,
        user.id,
    )
    .fetch_one(db)
    .await
    .map_err(|e| VialoError::Anyhow(e.into()))?
    .unwrap_or(false);

    if owns {
        Ok(())
    } else {
        Err(VialoError::Forbidden())
    }
}

#[utoipa::path(get, path = "/network/credentials", params(NetworkFilterOptions), responses((status = 200, description = "OK", body=Vec<CredentialModelWithAccountEmbed>)))]
pub async fn list_credentials(
    Query(opts): Query<NetworkFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user, AppRole::NetworkManager, &data.db).await?;
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    let records = conditional_query_as!(
        CredentialModelWithAccountEmbed,
        "SELECT nc.id, nc.username, nc.network_id, nc.last_updated,
        jsonb_build_object('id', nc.account_id, 'full_name', coalesce(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS account
        FROM net_cred nc LEFT JOIN accounts_people ap ON nc.account_id = ap.id LEFT JOIN account_groups ag ON nc.account_id = ag.id
         {#search} ORDER BY id ASC LIMIT {limit} OFFSET {offset}",
        #search = match opts.search {
            Some(src) => "WHERE account_id::text ILIKE '%' || {src} || '%'", //todo
            _ => ""
        }
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(records)))
}

#[utoipa::path(get, path = "/network/credentials/{id}", responses((status = 200, description = "OK", body=CredentialModelWithPasswordAndAccountEmbed)))]
pub async fn get_credential(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_own_or_network_manager(&user, id, &data.db).await?;

    let record = query_as!(
        CredentialModelWithPasswordAndAccountEmbed,
        r#"SELECT nc.id, nc.username, nc.password AS "password: Encrypted<String>", nc.network_id, nc.last_updated,
         jsonb_build_object('id', nc.account_id, 'full_name', coalesce(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account!: AccountEmbed"
         FROM net_cred nc
         LEFT JOIN accounts_people ap ON nc.account_id = ap.id
         LEFT JOIN account_groups ag ON nc.account_id = ag.id
         WHERE nc.id = $1"#,
        id
    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/network/credentials/{id}/mobileconfig/{filename}", responses((status = 200, description = "OK", content_type = "application/x-apple-aspen-config")))]
pub async fn get_mobileconfig(
    Path((id, _filename)): Path<(Uuid, String)>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_own_or_network_manager(&user, id, &data.db).await?;

    let record = query!(
        r#"SELECT nc.id, nc.username as "username!", nc.password AS "password: Encrypted<String>", nc.network_id, nc.last_updated,
         n.label AS network_label, n.ssid AS network_ssid, n.tls_trust, n.outer_identity,
         nc.account_id, coalesce(ap.full_name, ag.label) as account_full_name,
         nd.label as device_label
         FROM net_cred nc
         JOIN net_networks n ON nc.network_id = n.id
         LEFT JOIN accounts_people ap ON nc.account_id = ap.id
         LEFT JOIN account_groups ag ON nc.account_id = ag.id
         LEFT JOIN LATERAL (SELECT label FROM net_devices WHERE cred_id = nc.id LIMIT 1) nd ON NOT n.multi_device
         WHERE nc.id = $1 AND nc.username IS NOT NULL AND nc.password IS NOT NULL"#,
        id
    )
    .fetch_one(&data.db)
    .await?;
    let account_full_name = record.account_full_name.unwrap_or(record.id.to_string());
    let display_name = match record.device_label {
        Some(label) => format!("{account_full_name} \u{2014} {label}"),
        None => account_full_name,
    };
    let file = format!(
        include_str!("template.mobileconfig.xml"),
        network_label = record.network_label,
        account_full_name = display_name,
        username = record.username,
        password = record.password.map(|p| p.expose()).unwrap_or_default(),
        cred_id = record.id,
        network_id = record.network_id,
        network_ssid = record.network_ssid.as_deref().unwrap_or(&record.network_label),
        network_tlstrust = record.tls_trust.as_deref().unwrap_or(&data.config.org.domain),
        outer_identity = record.outer_identity.unwrap_or_else(|| format!("anonymous@{}", data.config.org.domain)),
        org_name = data.config.org.name
    );
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-apple-aspen-config")],
        file,
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct PutCredentialSchema {
    pub username: Option<String>,
    pub password: Option<String>,
}

#[utoipa::path(put, path = "/network/credentials/{id}", request_body = PutCredentialSchema, responses((status = 204, description = "Updated")))]
pub async fn put_credential(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<PutCredentialSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_own_or_network_manager(&user, id, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    // Encrypt the password if provided
    let password = body
        .password
        .as_ref()
        .map(encryption::encrypt)
        .transpose()?;

    let result = sqlx::query!(
        "UPDATE net_cred SET username = $1, password = $2 WHERE id = $3",
        body.username,
        password,
        id
    )
    .execute(&mut *conn)
    .await?;

    match result.rows_affected() {
        1 => Ok(StatusCode::NO_CONTENT),
        _ => Err(VialoError::NotFound()),
    }
}

#[utoipa::path(delete, path = "/network/credentials/{id}", responses((status = 204, description = "Deleted")))]
pub async fn delete_credential(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_own_or_network_manager(&user, id, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let rows_affected = sqlx::query!("DELETE FROM net_cred WHERE id = $1", id)
        .execute(&mut *conn)
        .await?
        .rows_affected();

    if rows_affected == 0 {
        return Err(VialoError::NotFound());
    }

    Ok(StatusCode::NO_CONTENT)
}
