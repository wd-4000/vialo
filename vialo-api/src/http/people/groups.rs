use serde_with::{NoneAsEmptyString, serde_as};
use utoipa::ToSchema;

use super::{
    models::{GroupListModel, GroupMemberModel},
    schemas::UserFilterOptions,
};
use crate::{
    AppState,
    http::{
        people::models::GroupGetModel,
        util::{JsonE, User, VialoError, grab_authd_conn_user, grab_trans},
    },
    permissions::{AppRole, GroupRole, check_app_role, check_manager_of_group_or_app_role},
};
use serde::{Deserialize, Serialize};
use sqlx::types::JsonValue;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use sqlx_conditional_queries::conditional_query_as;
use std::{collections::HashSet, sync::Arc};
use uuid::Uuid;

#[utoipa::path(get, path = "/people/groups", params(UserFilterOptions), responses((status = 200, description = "OK")))]
pub async fn list_groups(
    Query(opts): Query<UserFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);
    // let search = opts.search.unwrap_or("".to_string());
    let offset = (opts.page.unwrap_or(1) - 1) * limit;
    // This might be red in rust-analyzer. Ignore this.
    let record = conditional_query_as!(
        GroupListModel,
        "SELECT * FROM account_groups WHERE TRUE {#public_only} {#wh_search} LIMIT {limit} OFFSET {offset}",
        #public_only = match (check_app_role(user, crate::permissions::AppRole::AccountManager, &data.db).await.is_ok()){
                true => "",
                false => "AND public = true"
            },
        #wh_search = match (opts.search){
            Some(src) => "AND label ILIKE '%' || {src} || '%'",
            _ => ""
        }
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/people/groups/{group_id}/members", params(UserFilterOptions), responses((status = 200, description = "OK")))]
pub async fn list_group_members(
    Path(group_id): Path<Uuid>,
    Query(opts): Query<UserFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    // Private group members are only visible to group members themselves or AccountManagers.
    if check_app_role(user.clone(), AppRole::AccountManager, &data.db)
        .await
        .is_err()
    {
        let is_public =
            sqlx::query_scalar!("SELECT public FROM account_groups WHERE id = $1", group_id)
                .fetch_optional(&data.db)
                .await?
                .unwrap_or(false);

        if !is_public {
            let is_member = sqlx::query_scalar!(
                r#"SELECT COUNT(*) > 0 AS "is_member: bool" FROM account_group_memberships WHERE group_id = $1 AND account_id = $2"#,
                group_id,
                user.id
            )
            .fetch_one(&data.db)
            .await?
            .unwrap_or(false);

            if !is_member {
                return Err(VialoError::Forbidden());
            }
        }
    }
    let limit = opts.limit.unwrap_or(10);
    // let search = opts.search.unwrap_or("".to_string());
    let offset = (opts.page.unwrap_or(1) - 1) * limit;
    // This might be red in rust-analyzer. Ignore this.
    let record = conditional_query_as!(
        GroupMemberModel,
        r#"SELECT
            a.id as "id?",
            COALESCE(a.full_name, a.label, i.full_name) AS "full_name?",
            r.label AS "room_name?",
            r.id AS "room_id?",
            agm.role as "role: GroupRole"
        FROM
            accounts_people a
            FULL JOIN identities i ON i.id = a.auth_id
            LEFT JOIN res_leases l ON a.id = l.tenant_id
            LEFT JOIN res_rooms r ON l.room_id = r.id JOIN account_group_memberships agm ON a.id = agm.account_id WHERE agm.group_id = {group_id} {#wh_search} ORDER BY (a.id IS NULL) DESC, (i.id IS NULL) DESC, r.label DESC LIMIT {limit} OFFSET {offset}"#,
            #wh_search = match (opts.search){
                Some(src) => "AND COALESCE(a.full_name, a.label) ILIKE '%' || {src} || '%' OR r.label ILIKE '%' || {src} || '%'",
                _ => ""
            }
    )
        .fetch_all(&data.db)
        .await?;
    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/people/groups/members", params(UserFilterOptions), responses((status = 200, description = "OK")))]
pub async fn list_all_group_members(
    Query(opts): Query<UserFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);
    // let search = opts.search.unwrap_or("".to_string());
    let offset = (opts.page.unwrap_or(1) - 1) * limit;

    #[derive(Serialize)]
    pub struct GroupMemberReport {
        pub id: Uuid,
        pub label: String,
        pub icon: Option<String>,
        pub email: Option<String>,
        pub accounts: Option<Vec<JsonValue>>,
    }

    // This might be red in rust-analyzer. Ignore this.
    let record = conditional_query_as!(
        GroupMemberReport,
        "SELECT ag.id, ag.label, ag.icon, ag.email, array_agg(jsonb_build_object('id', cro.id, 'full_name', cro.full_name, 'room', (CASE WHEN cro.room_id IS NOT NULL THEN jsonb_build_object('id', cro.room_id, 'label', cro.room_name) ELSE NULL END))) as accounts FROM account_groups ag JOIN account_group_memberships agm on ag.id = agm.group_id JOIN current_room_occupants cro ON agm.account_id = cro.id {#public_only} GROUP BY ag.id LIMIT {limit} OFFSET {offset}",
        #public_only = match (check_app_role(user, crate::permissions::AppRole::AccountManager, &data.db).await.is_ok()){
                true => "",
                false => "WHERE ag.public = true"
            }

    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/people/groups/{id}", responses((status = 200, description = "OK")))]
pub async fn get_group(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let group = sqlx::query_as!(
        GroupGetModel,
        r#"SELECT ag.id, ag.label, ag.email, ag.icon, ag.public,  COALESCE(
                array_agg(agra.role) FILTER (WHERE agra.role IS NOT NULL),
                '{}'::app_role[]
            ) as "app_roles!: Vec<AppRole>" FROM account_groups ag LEFT JOIN account_group_app_roles agra ON ag.id = agra.group_id WHERE id = $1 GROUP BY ag.id"#,
        id
    )
    .fetch_one(&data.db)
    .await?;

    Ok(Json(group))
}

#[utoipa::path(delete, path = "/people/groups/{group_id}/members/{account_id}", responses((status = 200, description = "Deleted")))]
pub async fn delete_account(
    Path((group_id, account_id)): Path<(Uuid, Uuid)>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_manager_of_group_or_app_role(user.clone(), group_id, AppRole::AccountManager, &data.db)
        .await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        "DELETE FROM account_group_memberships WHERE group_id = $1 AND account_id = $2",
        group_id,
        account_id
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct AddUserToGroupSchema {
    pub account_id: Uuid,
    pub role: Option<GroupRole>,
}

#[utoipa::path(post, path = "/people/groups/{group_id}/members", request_body=AddUserToGroupSchema, responses((status = 200, description = "Created")))]
pub async fn add_account(
    Path(group_id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<AddUserToGroupSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_manager_of_group_or_app_role(user.clone(), group_id, AppRole::AccountManager, &data.db)
        .await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        "INSERT INTO account_group_memberships (group_id, account_id, role) VALUES ($1, $2, $3)",
        group_id,
        body.account_id,
        body.role.unwrap_or(GroupRole::Member) as GroupRole
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreateGroupSchema {
    pub label: String,
    #[serde_as(as = "NoneAsEmptyString")]
    pub email: Option<String>,
    pub icon: Option<String>,
    pub public: bool,
    pub app_roles: Option<HashSet<AppRole>>,
}

#[utoipa::path(post, path = "/people/groups", request_body=CreateGroupSchema, responses((status = 201, description = "Created")))]
pub async fn post_group(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreateGroupSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let new_group = sqlx::query_as!(
        GroupListModel,
        "INSERT INTO account_groups (label,email,public,icon) VALUES ($1,$2,$3,$4) returning *",
        body.label,
        body.email,
        body.public,
        body.icon
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok(Json(new_group))
}

#[utoipa::path(delete, path = "/people/groups/{id}", responses((status = 200, description = "Deleted")))]
pub async fn delete_group(
    Path(group_id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!("DELETE FROM account_groups WHERE id = $1", group_id)
        .execute(&mut *conn)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(put, path = "/people/groups/{id}", request_body=CreateGroupSchema, responses((status = 200, description = "Updated")))]
pub async fn put_group(
    Path(group_id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreateGroupSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_manager_of_group_or_app_role(user.clone(), group_id, AppRole::AccountManager, &data.db)
        .await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    sqlx::query!(
        "UPDATE account_groups SET label = $2, email = $3, public = $4, icon = $5 WHERE id = $1",
        group_id,
        body.label,
        body.email,
        body.public,
        body.icon
    )
    .execute(&mut *trans)
    .await?;

    // We want to change app roles
    if let Some(new_app_roles) = body.app_roles {
        let current_app_roles: HashSet<AppRole> = sqlx::query!(
            r#"SELECT role as "role: AppRole" FROM account_group_app_roles WHERE group_id = $1"#,
            group_id
        )
        .fetch_all(&mut *trans)
        .await?
        .iter()
        .map(|m| m.role.clone())
        .collect();

        // Look for roles that have been removed
        for current_role in &current_app_roles - &new_app_roles {
            sqlx::query!(
                "DELETE FROM account_group_app_roles WHERE group_id = $1 AND role = $2",
                group_id,
                current_role as AppRole
            )
            .execute(&mut *trans)
            .await?;
        }

        // Look for roles that have been added
        for new_role in &new_app_roles - &current_app_roles {
            sqlx::query!(
                "INSERT INTO account_group_app_roles (group_id, role) VALUES ($1, $2)",
                group_id,
                new_role as AppRole
            )
            .execute(&mut *trans)
            .await?;
        }
    }

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PatchUserInGroupSchema {
    pub role: GroupRole,
}

#[utoipa::path(patch, path = "/people/groups/{group_id}/members/{account_id}", request_body=PatchUserInGroupSchema, responses((status = 200, description = "Updated")))]
pub async fn patch_account(
    Path((group_id, account_id)): Path<(Uuid, Uuid)>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PatchUserInGroupSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_manager_of_group_or_app_role(user.clone(), group_id, AppRole::AccountManager, &data.db)
        .await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        "UPDATE account_group_memberships SET role = $3 WHERE group_id = $1 AND account_id = $2",
        group_id,
        account_id,
        body.role as GroupRole
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
