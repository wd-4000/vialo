use serde_with::{NoneAsEmptyString, serde_as};
use sqlx::{PgConnection, PgExecutor, Postgres, Transaction};
use utoipa::ToSchema;

use super::{
    models::{GroupListModel, GroupMemberModel, GroupSourceRule, GroupSourceType},
    schemas::{GroupFilterOptions, GroupMemberFilterOptions},
};
use crate::{
    AppState,
    helpers::{I18nMap, LangVariant},
    http::{
        people::models::{GroupGetModel, GroupGetModelAllLanguages},
        util::{
            JsonE, User, VialoError, clamp_pagination, get_i18n_arg_arrays, grab_authd_conn_user,
            grab_trans, insert_i18n_strings, is_unique_violation,
            models::{AccountPersonEmbed, RoomEmbed},
        },
    },
    permissions::{AppRole, GroupRole, check_app_role, check_manager_of_group_or_app_role},
};
use serde::{Deserialize, Serialize};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use sqlx_conditional_queries::conditional_query_as;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;

/// An empty map means "unset", and would trip up the i18n SQL functions.
fn i18n_map(map: Option<I18nMap>) -> Option<HashMap<String, String>> {
    map.map(HashMap::from).filter(|m| !m.is_empty())
}

#[utoipa::path(get, path = "/people/groups", params(GroupFilterOptions), responses((status = 200, description = "OK", body=Vec<GroupListModel>)))]
pub async fn list_groups(
    Query(opts): Query<GroupFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);
    // This might be red in rust-analyzer. Ignore this.
    let record = conditional_query_as!(
        GroupListModel,
        "SELECT
            id,
            label,
            email,
            icon,
            public,
            get_i18n_string(description_i18n, {langs:Vec<String>}) AS description
        FROM account_groups WHERE TRUE {#public_only} {#wh_search} LIMIT {limit} OFFSET {offset}",
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

#[utoipa::path(get, path = "/people/groups/{group_id}/members", params(GroupMemberFilterOptions), responses((status = 200, description = "OK", body=Vec<GroupMemberModel>)))]
pub async fn list_group_members(
    Path(group_id): Path<Uuid>,
    Query(opts): Query<GroupMemberFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    if opts.include_source {
        check_manager_of_group_or_app_role(
            user.clone(),
            group_id,
            AppRole::AccountManager,
            &data.db,
        )
        .await?;
    }

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
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    // This might be red in rust-analyzer. Ignore this.
    let record = conditional_query_as!(
        GroupMemberModel,
        r#"SELECT
            a.id as "id!",
            COALESCE(a.full_name, a.label, i.full_name) AS "full_name?",
            CASE WHEN r.id IS NOT NULL THEN jsonb_build_object('label', r.label, 'id', r.id) ELSE NULL END AS "room: RoomEmbed",
            agm.role as "role!: GroupRole",
            {#source}
        FROM
            accounts_people a
            FULL JOIN identities i ON i.id = a.auth_id
            LEFT JOIN res_leases l ON a.id = l.tenant_id
            LEFT JOIN res_rooms r ON l.room_id = r.id JOIN account_group_memberships agm ON a.id = agm.account_id WHERE agm.group_id = {group_id} {#wh_search} ORDER BY (a.id IS NULL) DESC, (i.id IS NULL) DESC, r.label DESC LIMIT {limit} OFFSET {offset}"#,
            #source = match opts.include_source {
                true => r#"(SELECT d.source FROM account_group_memberships_derived d WHERE d.account_id = agm.account_id AND d.group_id = agm.group_id) AS "source: GroupSourceType""#,
                false => r#"NULL::group_source_type AS "source: GroupSourceType""#,
            },
            #wh_search = match opts.search {
                Some(src) => "AND COALESCE(a.full_name, a.label) ILIKE '%' || {src} || '%' OR r.label ILIKE '%' || {src} || '%'",
                _ => ""
            }
    )
        .fetch_all(&data.db)
        .await?;
    Ok((StatusCode::OK, Json(record)))
}

#[derive(Serialize, ToSchema)]
pub struct GroupMemberReport {
    pub id: Uuid,
    pub label: String,
    pub icon: Option<String>,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub accounts: Option<Vec<AccountPersonEmbed>>,
    pub description: Option<String>,
}

#[utoipa::path(get, path = "/people/groups/overview", params(GroupFilterOptions), responses((status = 200, description = "OK", body = Vec<GroupMemberReport>)))]
pub async fn list_all_group_overview(
    Query(opts): Query<GroupFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let record = conditional_query_as!(
        GroupMemberReport,
        r#"SELECT ag.id, ag.label, ag.icon, ag.email, get_i18n_string(ag.description_i18n, {langs:Vec<String>}) AS description, array_agg(jsonb_build_object('id', cro.id, 'full_name', cro.full_name, 'room', (CASE WHEN cro.room_id IS NOT NULL THEN jsonb_build_object('id', cro.room_id, 'label', cro.room_name) ELSE NULL END))) as "accounts: Vec<AccountPersonEmbed>" FROM account_groups ag JOIN account_group_memberships agm on ag.id = agm.group_id JOIN current_room_occupants cro ON agm.account_id = cro.id {#public_only} GROUP BY ag.id LIMIT {limit} OFFSET {offset}"#,
        #public_only = match (check_app_role(user, crate::permissions::AppRole::AccountManager, &data.db).await.is_ok()){
                true => "WHERE TRUE",
                false => "WHERE ag.public = true"
            },

    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/people/groups/{id}/overview", params(GroupFilterOptions), responses((status = 200, description = "OK", body = GroupMemberReport)))]
pub async fn get_group_overview(
    Path(id): Path<Uuid>,
    Query(opts): Query<GroupFilterOptions>,
    Extension(user_o): Extension<Option<User>>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let is_manager = match &user_o {
        Some(u) => check_app_role(u.clone(), AppRole::AccountManager, &data.db)
            .await
            .is_ok(),
        None => false,
    };

    // This might be red in rust-analyzer. Ignore this.
    let record =   conditional_query_as!(
        GroupMemberReport,
        r#"SELECT ag.id, ag.label, ag.icon, ag.email, get_i18n_string(ag.description_i18n, {langs:Vec<String>}) AS description, {#include_members} FROM account_groups ag JOIN account_group_memberships agm on ag.id = agm.group_id JOIN current_room_occupants cro ON agm.account_id = cro.id {#public_only} AND ag.id = {id} GROUP BY ag.id"#,
        #include_members = match user_o {
            Some(_) => r#"array_agg(jsonb_build_object('id', cro.id, 'full_name', cro.full_name, 'room', (CASE WHEN cro.room_id IS NOT NULL THEN jsonb_build_object('id', cro.room_id, 'label', cro.room_name) ELSE NULL END))) as "accounts: Vec<AccountPersonEmbed>""#,
            None => r#"null as "accounts: Vec<AccountPersonEmbed>""#,
        },
        #public_only = match is_manager {
            true => "WHERE TRUE",
            false => "WHERE ag.public = true",
        },

    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

/// Private groups are only visible to AccountManagers.
async fn check_group_visible<'c, T: PgExecutor<'c>>(
    user: User,
    public: bool,
    db: T,
) -> Result<(), VialoError> {
    if !public
        && check_app_role(user, AppRole::AccountManager, db)
            .await
            .is_err()
    {
        return Err(VialoError::Forbidden());
    }

    Ok(())
}

#[utoipa::path(get, path = "/people/groups/{id}", params(GroupFilterOptions), responses((status = 200, description = "OK", body=LangVariant<GroupGetModelAllLanguages, GroupGetModel>)))]
pub async fn get_group(
    Path(id): Path<Uuid>,
    Query(opts): Query<GroupFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    if langs == ["all"] {
        let group = sqlx::query_as!(
            GroupGetModelAllLanguages,
            r#"SELECT ag.id, ag.label, ag.email, ag.icon, ag.public, get_i18n_all_string_translations(ag.description_i18n) AS description, COALESCE(
                array_agg(agra.role) FILTER (WHERE agra.role IS NOT NULL),
                '{}'::app_role[]
            ) as "app_roles!: Vec<AppRole>", get_group_source_rules(ag.id) AS "source_rules!: Vec<GroupSourceRule>" FROM account_groups ag LEFT JOIN account_group_app_roles agra ON ag.id = agra.group_id WHERE id = $1 GROUP BY ag.id"#,
            id
        )
        .fetch_one(&data.db)
        .await?;

        check_group_visible(user, group.public, &data.db).await?;

        Ok(Json(LangVariant::AllLangs(group)))
    } else {
        let group = sqlx::query_as!(
            GroupGetModel,
            r#"SELECT ag.id, ag.label, ag.email, ag.icon, ag.public, get_i18n_string(ag.description_i18n, $2) AS description, COALESCE(
                array_agg(agra.role) FILTER (WHERE agra.role IS NOT NULL),
                '{}'::app_role[]
            ) as "app_roles!: Vec<AppRole>", get_group_source_rules(ag.id) AS "source_rules!: Vec<GroupSourceRule>" FROM account_groups ag LEFT JOIN account_group_app_roles agra ON ag.id = agra.group_id WHERE id = $1 GROUP BY ag.id"#,
            id,
            &langs
        )
        .fetch_one(&data.db)
        .await?;

        check_group_visible(user, group.public, &data.db).await?;

        Ok(Json(LangVariant::Localized(group)))
    }
}

#[utoipa::path(delete, path = "/people/groups/{group_id}/members/{account_id}", responses((status = 204, description = "Deleted")))] // no body
pub async fn delete_account(
    Path((group_id, account_id)): Path<(Uuid, Uuid)>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_manager_of_group_or_app_role(user.clone(), group_id, AppRole::AccountManager, &data.db)
        .await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let rows_affected = sqlx::query!(
        "DELETE FROM account_group_memberships_manual WHERE group_id = $1 AND account_id = $2",
        group_id,
        account_id
    )
    .execute(&mut *conn)
    .await?
    .rows_affected();

    if rows_affected == 0 {
        return Err(
            match is_derived_member(&mut *conn, group_id, account_id).await? {
                true => VialoError::AppError(StatusCode::CONFLICT, "membership_derived".into()),
                false => VialoError::NotFound(),
            },
        );
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct AddUserToGroupSchema {
    pub account_id: Uuid,
    pub role: Option<GroupRole>,
}

#[utoipa::path(post, path = "/people/groups/{group_id}/members", request_body=AddUserToGroupSchema, responses((status = 204, description = "Created")))] // no body
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
        "INSERT INTO account_group_memberships_manual (group_id, account_id, role) VALUES ($1, $2, $3)",
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
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub label: String,
    #[serde_as(as = "NoneAsEmptyString")]
    #[schema(format = Email)]
    pub email: Option<String>,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_64")]
    pub icon: Option<String>,
    pub public: bool,
    pub app_roles: Option<HashSet<AppRole>>,
    /// NULL = leave unchanged
    pub source_rules: Option<Vec<GroupSourceRule>>,
    pub description: Option<I18nMap>,
}

#[utoipa::path(post, path = "/people/groups", request_body=CreateGroupSchema, responses((status = 201, description = "Created", body=GroupListModel)))]
pub async fn post_group(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreateGroupSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let new_group = insert_group(&mut trans, body).await?;

    trans.commit().await?;

    Ok(Json(new_group))
}

/// Inserts a group including app roles & rules. The caller checks permissions.
pub async fn insert_group(
    trans: &mut Transaction<'_, Postgres>,
    body: CreateGroupSchema,
) -> Result<GroupListModel, VialoError> {
    let processed_i18n_fields = insert_i18n_strings(
        &mut *trans,
        vec![("description", i18n_map(body.description))],
    )
    .await?;

    let langs = vec![String::from("en"), String::from("de")];

    let new_group = sqlx::query_as!(
        GroupListModel,
        "INSERT INTO account_groups (label,email,public,icon,description_i18n) VALUES ($1,$2,$3,$4,$5) returning id, label, email, icon, public, get_i18n_string(description_i18n, $6) AS description",
        body.label,
        body.email,
        body.public,
        body.icon,
        processed_i18n_fields.get("description"),
        &langs
    )
    .fetch_one(&mut **trans)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            VialoError::AppError(StatusCode::CONFLICT, "email_conflict".into())
        } else {
            e.into()
        }
    })?;

    if let Some(app_roles) = body.app_roles {
        replace_app_roles(&mut *trans, new_group.id, app_roles).await?;
    }

    if let Some(source_rules) = body.source_rules {
        replace_source_rules(&mut *trans, new_group.id, source_rules).await?;
    }

    Ok(new_group)
}

async fn current_app_roles(
    db: &mut PgConnection,
    group_id: Uuid,
) -> Result<HashSet<AppRole>, VialoError> {
    Ok(sqlx::query!(
        r#"SELECT role as "role: AppRole" FROM account_group_app_roles WHERE group_id = $1"#,
        group_id
    )
    .fetch_all(&mut *db)
    .await?
    .into_iter()
    .map(|m| m.role)
    .collect())
}

/// Sets a group's app roles to exactly `new_app_roles`. The caller checks permissions.
pub async fn replace_app_roles(
    db: &mut PgConnection,
    group_id: Uuid,
    new_app_roles: HashSet<AppRole>,
) -> Result<(), VialoError> {
    let current_app_roles = current_app_roles(&mut *db, group_id).await?;

    // Look for roles that have been removed
    for current_role in &current_app_roles - &new_app_roles {
        sqlx::query!(
            "DELETE FROM account_group_app_roles WHERE group_id = $1 AND role = $2",
            group_id,
            current_role as AppRole
        )
        .execute(&mut *db)
        .await?;
    }

    // Look for roles that have been added
    for new_role in &new_app_roles - &current_app_roles {
        sqlx::query!(
            "INSERT INTO account_group_app_roles (group_id, role) VALUES ($1, $2)",
            group_id,
            new_role as AppRole
        )
        .execute(&mut *db)
        .await?;
    }

    Ok(())
}

/// Sets a group's membership rules to exactly `source_rules`. The caller checks permissions.
pub async fn replace_source_rules(
    db: &mut PgConnection,
    group_id: Uuid,
    source_rules: Vec<GroupSourceRule>,
) -> Result<(), VialoError> {
    let mut source_groups = Vec::new();
    for source_rule in &source_rules {
        match source_rule {
            GroupSourceRule::Floor { .. } => {}
            GroupSourceRule::GroupId { group_id, .. } => source_groups.push(*group_id),
            GroupSourceRule::GroupType { group_type, .. } => {
                if *group_type != GroupSourceType::Floor {
                    return Err(VialoError::AppError(
                        StatusCode::BAD_REQUEST,
                        "nested_rule".into(),
                    ));
                }
            }
        }
    }

    // Rules only go one level deep: no group_id rules reading groups that have group_id/group_type rules themselves
    if !source_groups.is_empty() {
        let nested = sqlx::query_scalar!(
            r#"SELECT (
                $1::uuid = ANY($2::uuid[])
                OR EXISTS (SELECT 1 FROM account_group_source_rules WHERE type != 'floor' AND group_id = ANY($2))
                OR EXISTS (SELECT 1 FROM account_group_source_rules WHERE source_group_id = $1)
            ) AS "nested!""#,
            group_id,
            &source_groups
        )
        .fetch_one(&mut *db)
        .await?;

        if nested {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "nested_rule".into(),
            ));
        }
    }

    sqlx::query!(
        "DELETE FROM account_group_source_rules WHERE group_id = $1",
        group_id
    )
    .execute(&mut *db)
    .await?;

    sqlx::query!(
        r#"INSERT INTO account_group_source_rules (group_id, type, floor, source_group_id, source_group_type, source_role)
        SELECT $1, r.type, r.floor, r.group_id, r.group_type, r.role
        FROM jsonb_to_recordset($2) AS r(type group_source_type, floor int, group_id uuid, group_type group_source_type, role group_role)"#,
        group_id,
        sqlx::types::Json(&source_rules) as _
    )
    .execute(&mut *db)
    .await
    .map_err(|e| match e.as_database_error() {
        Some(d) if d.is_foreign_key_violation() => {
            VialoError::AppError(StatusCode::BAD_REQUEST, "unknown_source_group".into())
        }
        _ => e.into(),
    })?;

    Ok(())
}

#[utoipa::path(delete, path = "/people/groups/{id}", responses((status = 204, description = "Deleted")))] // no body
pub async fn delete_group(
    Path(group_id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let deleted = sqlx::query!(
        "DELETE FROM account_groups WHERE id = $1 RETURNING description_i18n",
        group_id
    )
    .fetch_optional(&mut *trans)
    .await?;

    if let Some(group) = deleted {
        sqlx::query!(
            "DELETE FROM i18n_strings WHERE id = $1",
            group.description_i18n
        )
        .execute(&mut *trans)
        .await?;
    }

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(put, path = "/people/groups/{id}", request_body=CreateGroupSchema, responses((status = 204, description = "Updated")))] // no body
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

    let (description_langs, description_contents) = get_i18n_arg_arrays(i18n_map(body.description));

    sqlx::query!(
        r#"UPDATE account_groups SET label = $2, email = $3, public = $4, icon = $5,
            description_i18n = CASE
                WHEN cardinality($6::text[]) > 0 THEN update_i18n_strings(description_i18n, $6, $7)
                ELSE description_i18n
            END
        WHERE id = $1"#,
        group_id,
        body.label,
        body.email,
        body.public,
        body.icon,
        &description_langs,
        &description_contents
    )
    .execute(&mut *trans)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            VialoError::AppError(StatusCode::CONFLICT, "email_conflict".into())
        } else {
            e.into()
        }
    })?;

    // Group managers may edit their group, but not what it grants (something something privilege escalation)

    // We want to change app roles
    if let Some(new_app_roles) = body.app_roles
        && current_app_roles(&mut trans, group_id).await? != new_app_roles
    {
        check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
        replace_app_roles(&mut trans, group_id, new_app_roles).await?;
    }

    // We want to change the rules (hell yes)
    if let Some(new_source_rules) = body.source_rules {
        let current_source_rules = sqlx::query_scalar!(
            r#"SELECT get_group_source_rules($1) AS "source_rules!: Vec<GroupSourceRule>""#,
            group_id
        )
        .fetch_one(&mut *trans)
        .await?;

        if current_source_rules != new_source_rules {
            check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
            replace_source_rules(&mut trans, group_id, new_source_rules).await?;
        }
    }

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct PatchUserInGroupSchema {
    pub role: GroupRole,
}

#[utoipa::path(patch, path = "/people/groups/{group_id}/members/{account_id}", request_body=PatchUserInGroupSchema, responses((status = 204, description = "Updated")))] // no body
pub async fn patch_account(
    Path((group_id, account_id)): Path<(Uuid, Uuid)>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PatchUserInGroupSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_manager_of_group_or_app_role(user.clone(), group_id, AppRole::AccountManager, &data.db)
        .await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    // Derived membership already makes them a member, so drop the manual override
    if body.role == GroupRole::Member && is_derived_member(&mut *conn, group_id, account_id).await?
    {
        sqlx::query!(
            "DELETE FROM account_group_memberships_manual WHERE group_id = $1 AND account_id = $2",
            group_id,
            account_id
        )
        .execute(&mut *conn)
        .await?;
        return Ok(StatusCode::NO_CONTENT);
    }

    // Upsert, as derived members have no manual row yet
    let rows_affected = sqlx::query!(
        "INSERT INTO account_group_memberships_manual (group_id, account_id, role)
         SELECT $1, $2, $3 WHERE EXISTS (SELECT 1 FROM account_group_memberships WHERE group_id = $1 AND account_id = $2)
         ON CONFLICT (account_id, group_id) DO UPDATE SET role = EXCLUDED.role",
        group_id,
        account_id,
        body.role as GroupRole
    )
    .execute(&mut *conn)
    .await?
    .rows_affected();

    if rows_affected == 0 {
        return Err(VialoError::NotFound());
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Whether the membership comes from an account_group_source_rules rule
async fn is_derived_member<'c, T: PgExecutor<'c>>(
    db: T,
    group_id: Uuid,
    account_id: Uuid,
) -> Result<bool, VialoError> {
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM account_group_memberships_derived WHERE group_id = $1 AND account_id = $2) AS "derived!""#,
        group_id,
        account_id
    )
    .fetch_one(db)
    .await?)
}
