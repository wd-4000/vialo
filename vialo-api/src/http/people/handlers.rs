use super::{
    models::{
        AccountModel, AccountModelForOverview, CreatePersonResponse, CurrentRoomOccupantsViewModel,
        GroupStubListModelWithRole, LeaseModelWithEmbeddedSublease, PeopleLookupModel,
        PersonOverviewModel, PersonOverviewNetworkModel, PersonalTransactionModel,
        TransactionStatus,
    },
    schemas::{CreateUserSchema, UserFilterOptions},
};

#[cfg(feature = "printer")]
use crate::printer::{self, models::JobData};

use crate::{
    AppState, helpers,
    http::util::{
        JsonE, User, VialoError, clamp_pagination, grab_authd_conn_user, grab_trans,
        models::{AccountEmbed, AccountPersonEmbed, GroupEmbed, ProductType, RoomEmbed},
    },
};
use crate::{
    helpers::{PgDate, encryption},
    permissions::check_app_role,
};
use crate::{http::people::models::PersonalNetRealmOverviewModel, permissions::AppRole};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use rand::{
    distr::{Alphanumeric, SampleString},
    rng,
};
use serde::Serialize;
use serde_json::json;
use sqlx::{prelude::FromRow, query};
use sqlx_conditional_queries::conditional_query_as;
use std::{ops::DerefMut, sync::Arc};
use tokio::try_join;
use utoipa::ToSchema;
use uuid::Uuid;

// "SELECT cl.id, from_account, to_account, credits, cl.created_at,
//     CASE
//     WHEN ac.full_name IS NOT NULL THEN jsonb_build_object(
//         'type','account',
//         'id', ac.id,
//         'full_name', ac.full_name
//     )
//     ELSE jsonb_build_object(
//         'type','subsystem',
//         'id', tl.caused_by_subsystem
//     )
//     END AS caused_by
// FROM credit_ledger cl
// JOIN table_log tl ON record_id = cl.id
// LEFT JOIN accounts_people ac ON caused_by_account = ac.id
// WHERE (to_account = $1 OR from_account = $1) AND tl.table_name = 'credit_ledger' ORDER BY created_at LIMIT 3", id)
// .fetch_all(&data.db)

#[utoipa::path(get, path = "/people", description = "List identities and people. IDs might be null because this endpoint also lists identities with no attached people", params(UserFilterOptions), responses((status = 200, description = "OK", body=Vec<CurrentRoomOccupantsViewModel>)))]
pub async fn list_people(
    Query(opts): Query<UserFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user, AppRole::AccountManager, &data.db).await?;

    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    // This might be red in rust-analyzer. Ignore this.
    let record = conditional_query_as!(
        CurrentRoomOccupantsViewModel,
        r#"SELECT
            a.id as "id?",
            i.id as "auth_id?",
            COALESCE(a.full_name, a.label, i.full_name) AS "full_name?",
            CASE WHEN r.id IS NOT NULL THEN jsonb_build_object('label', r.label, 'id', r.id) END AS "room: RoomEmbed"
        FROM
            accounts_people a
            FULL JOIN identities i ON i.id = a.auth_id
            LEFT JOIN res_leases l ON a.id = l.tenant_id
            LEFT JOIN res_rooms r ON l.room_id = r.id {#j_wh_group} {#wh_search} {#wh_resident} ORDER BY (a.id IS NULL) DESC, (i.id IS NULL) DESC, r.label DESC LIMIT {limit} OFFSET {offset}"#,
            #j_wh_group = match(opts.group) {
                Some(groups) => "JOIN account_group_memberships agm ON a.id = agm.account_id WHERE agm.group_id = ANY({groups:Vec<Uuid>})",
                None => "WHERE TRUE"
            },
            #wh_search = match (opts.search){
                Some(src) => "AND COALESCE(a.full_name, a.label) ILIKE '%' || {src} || '%' OR r.label ILIKE '%' || {src} || '%'",
                _ => ""
            },
            #wh_resident = match (opts.resident.unwrap_or(false)){
            true => "AND r.label IS NOT NULL",
            _ => ""
            }
    )
        .fetch_all(&data.db)
        .await?;
    Ok((StatusCode::OK, Json(record)))
}

async fn get_person_roles_impl(
    target_id: Uuid,
    data: Arc<AppState>,
) -> Result<impl IntoResponse, VialoError> {
    // permission check in the caller!

    let roles: Vec<AppRole> = sqlx::query_scalar!(
        r#"SELECT agra.role as "role: AppRole" FROM account_group_memberships agm JOIN account_group_app_roles agra ON
                agm.group_id = agra.group_id WHERE agm.account_id = $1"#,
        target_id
    )
    .fetch_all(&data.db)
    .await?;

    Ok(Json(roles))
}

#[utoipa::path(get, path = "/people/lookup", description = "List people in a minimal format. Only confirmed, no solitary identities.", params(UserFilterOptions), responses((status = 200, description = "OK", body=Vec<PeopleLookupModel>)))]
pub async fn lookup_people(
    Query(opts): Query<UserFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user, AppRole::AccountManager, &data.db).await?;

    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;
    // This might be red in rust-analyzer. Ignore this.
    let record = conditional_query_as!(
        PeopleLookupModel,
        r#"SELECT
            a.id as "id",
            COALESCE(a.full_name, a.label) AS "full_name?",
            jsonb_build_object('label', r.label, 'id', r.id) AS "room: RoomEmbed"
        FROM
            accounts_people a
            LEFT JOIN res_leases l ON a.id = l.tenant_id
            LEFT JOIN res_rooms r ON l.room_id = r.id {#j_wh_group} {#wh_search} {#wh_resident} ORDER BY r.label DESC LIMIT {limit} OFFSET {offset}"#,
            #j_wh_group = match(opts.group) {
                Some(groups) => "JOIN account_group_memberships agm ON a.id = agm.account_id WHERE agm.group_id = ANY({groups:Vec<Uuid>})",
                None => "WHERE TRUE"
            },
            #wh_search = match (opts.search){
                Some(src) => "AND COALESCE(a.full_name, a.label) ILIKE '%' || {src} || '%' OR r.label ILIKE '%' || {src} || '%'",
                _ => ""
            },
            #wh_resident = match (opts.resident.unwrap_or(false)){
            true => "AND r.label IS NOT NULL",
            _ => ""
            }
    )
        .fetch_all(&data.db)
        .await?;
    Ok((StatusCode::OK, Json(record)))
}

#[utoipa::path(get, path = "/people/me/roles", responses((status = 200, description = "OK", body=Vec<AppRole>)))]
pub async fn get_person_roles_me(
    Extension(User { id }): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    get_person_roles_impl(id, data).await
}

#[utoipa::path(get, path = "/people/{id}/roles", responses((status = 200, description = "OK", body=Vec<AppRole>)))]
pub async fn get_person_roles_by_id(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    if user.id != id {
        check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    }
    get_person_roles_impl(id, data).await
}

#[derive(Serialize, ToSchema)]
pub struct AccountCapabilities {
    pub roles: Vec<AppRole>,
    // These boolean flags represent "derived" access based on group memberships
    pub can_manage_some_bookables: bool,
    pub can_interact_with_some_boards: bool,
}

async fn get_person_capabilities_impl(
    target_id: Uuid,
    data: Arc<AppState>,
) -> Result<impl IntoResponse, VialoError> {
    let roles_future = sqlx::query_scalar!(
        r#"SELECT agra.role as "role: AppRole"
           FROM account_group_memberships agm
           JOIN account_group_app_roles agra ON agm.group_id = agra.group_id
           WHERE agm.account_id = $1"#,
        target_id
    )
    .fetch_all(&data.db);

    // Can they manage *any* bookable? (Are they in a group that has BookableManager or have >= admin)
    let bookable_future = sqlx::query_scalar!(
        r#"SELECT (
            -- Bookable Manager
            account_role_exists($1, 'bookable_manager')
            OR
            -- New type
            (SELECT COUNT(*) > 0 FROM account_group_memberships agm
                JOIN bookable_asset_type_group_perms batgp ON agm.group_id = batgp.group_id
                WHERE agm.account_id = $1 AND batgp.perm >= 'admin'::bookable_perm)
        ) AS "allowed!: bool" "#,
        target_id
    )
    .fetch_one(&data.db);

    // Can they moderate *any* board?
    let board_future = sqlx::query_scalar!(
        r#"SELECT EXISTS (
             SELECT 1 FROM board_group_perms bgp
             JOIN account_group_memberships agm ON bgp.group_id = agm.group_id
             WHERE agm.account_id = $1 AND bgp.perm >= 'moderate'
           ) AS "exists!""#,
        target_id
    )
    .fetch_one(&data.db);

    let (roles, can_manage_some_bookables, can_interact_with_some_boards) =
        tokio::try_join!(roles_future, bookable_future, board_future)?;

    Ok(Json(AccountCapabilities {
        roles,
        can_manage_some_bookables,
        can_interact_with_some_boards,
    }))
}

#[utoipa::path(get, path = "/people/me/capabilities", description = "Used in the admin panel to detect which navbar items to show (see useConditionalUI)" ,responses((status = 200, description = "OK", body=AccountCapabilities)))]
pub async fn get_person_capabilities_me(
    Extension(User { id }): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    get_person_capabilities_impl(id, data).await
}

#[utoipa::path(get, path = "/people/{id}/groups", responses((status = 200, description = "OK", body=Vec<GroupStubListModelWithRole>)))]
pub async fn get_person_groups(
    Path(id_path): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    if user.id != id_path {
        check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    }

    let groups = sqlx::query_as!(GroupStubListModelWithRole, "select id, label, role::text from account_group_memberships agm join account_groups ag on agm.group_id = ag.id WHERE account_id = $1", id_path)
        .fetch_all(&data.db).await?;
    Ok(Json(groups))
}

#[derive(Serialize, Debug, FromRow, ToSchema)]
pub struct PersonAppRoleReport {
    pub groups: Option<Vec<GroupEmbed>>,
    pub role: Option<AppRole>,
}

#[utoipa::path(get, path = "/people/{id}/app_roles", responses((status = 200, description = "OK", body=Vec<PersonAppRoleReport>)))]
pub async fn get_person_app_roles(
    Path(id_path): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    if user.id != id_path {
        check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    }

    let groups = sqlx::query_as!(PersonAppRoleReport, r#"select array_agg(jsonb_build_object('id',agm.group_id, 'label',ag.label)) as "groups: Vec<GroupEmbed>", agra.role as "role: AppRole" from account_group_memberships agm join account_group_app_roles agra on agm.group_id = agra.group_id join account_groups ag on agra.group_id = ag.id where agm.account_id = $1 group by agra.role;"#, id_path)
        .fetch_all(&data.db).await?;
    Ok(Json(groups))
}

async fn get_person_impl(
    target_id: Uuid,
    data: Arc<AppState>,
) -> Result<impl IntoResponse, VialoError> {
    // permission check in the caller!

    let person = conditional_query_as!(
        AccountModel,
        r#"SELECT    id,
       label,
     auth_id,
      email,
     membership_start,
       membership_end as "membership_end: PgDate",
        full_name,
        manually_suspended,
       credit_balance FROM accounts_people WHERE id = {target_id}"#
    )
    .fetch_one(&data.db)
    .await?;

    Ok(Json(person))
}

#[utoipa::path(get, path = "/people/me", responses((status = 200, description = "OK", body=AccountModel)))]
pub async fn get_me(
    Extension(User { id: user_id }): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    get_person_impl(user_id, data).await
}

#[utoipa::path(get, path = "/people/{id}", responses((status = 200, description = "OK", body=AccountModel)))]
pub async fn get_person(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    if user.id != id {
        check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;
    }

    get_person_impl(id, data).await
}

#[utoipa::path(get, path = "/people/{id}/overview", responses((status = 200, description = "OK", body=PersonOverviewModel)))]
pub async fn get_person_overview(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    // theoretically nothing prevents us from also letting regular users query this but I don't think it's needed
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;

    let account_task;

    // what if conditional_query_as was evil
    cfg_if::cfg_if! {
    if #[cfg(feature = "printer")] {
         account_task = sqlx::query_as!(AccountModelForOverview, r#"SELECT ap.id,
         ap.label,
       ap.auth_id,
        ap.email,
       ap.membership_start,
         ap.membership_end as "membership_end: PgDate",
          ap.full_name,
          ap.manually_suspended,
         ap.credit_balance, spc.printer_username, spc.printer_id as "printer_id?"
         FROM accounts_people ap LEFT JOIN subsystem_printer_context spc ON ap.id = spc.id WHERE ap.id = $1"#, id).fetch_one(&data.db);

         } else {
              account_task =  sqlx::query_as!(AccountModelForOverview, r#"SELECT ap.id,
             ap.label,
           ap.auth_id,
            ap.email,
           ap.membership_start,
             ap.membership_end as "membership_end: PgDate",
              ap.full_name,
              ap.manually_suspended,
             ap.credit_balance FROM accounts_people ap WHERE ap.id = $1"#, id).fetch_one(&data.db);
      }
      }
    let q = try_join!(
        account_task,
         sqlx::query_as!(GroupStubListModelWithRole, "select id, label, role::text from account_group_memberships agm join account_groups ag on agm.group_id = ag.id WHERE account_id = $1", id)
            .fetch_all(&data.db),
        sqlx::query_as!(
            LeaseModelWithEmbeddedSublease,
            r#"SELECT rl.id, rl.begins, rl.ends, rl.tenant_id,
            jsonb_build_object('label', rr.label, 'id', rr.id) AS "room: RoomEmbed",
            CASE
                WHEN rl.lessor_id IS NOT NULL THEN jsonb_build_object(
                    'id', rl.lessor_id,
                    'full_name', ac.full_name
                )
                ELSE NULL
            END AS "lessor: AccountPersonEmbed"
            FROM res_leases rl JOIN res_rooms rr ON rl.room_id = rr.id LEFT JOIN accounts_people ac ON rl.lessor_id = ac.id WHERE tenant_id = $1 LIMIT 1"#,
            id
        )
        .fetch_optional(&data.db),
        sqlx::query_as!(PersonalTransactionModel,
            r#"SELECT cl.id,
            CASE WHEN from_account IS NOT NULL THEN jsonb_build_object('id', from_account, 'full_name', ac_from.full_name, 'type', 'person') ELSE NULL END AS "from_account: AccountEmbed",
            CASE WHEN to_account IS NOT NULL THEN jsonb_build_object('id', to_account, 'full_name', ac_to.full_name, 'type', 'person') ELSE NULL END AS "to_account: AccountEmbed",
             credits, cl.created_at, cl.last_updated, cl.status as "status: TransactionStatus", cl.product as "product: ProductType", cl.refund_of
            FROM credit_ledger cl
            LEFT JOIN accounts_people ac_from ON from_account = ac_from.id
            LEFT JOIN accounts_people ac_to ON to_account = ac_to.id
            WHERE to_account = $1 OR from_account = $1
            ORDER BY created_at
            LIMIT 5"#, id)
            .fetch_all(&data.db),

    sqlx::query_as!(PersonalNetRealmOverviewModel, "select nr.ipv4_nat, nr.id from net_realm_assignments nra JOIN net_realms nr ON nra.realm_id = nr.id WHERE nra.account_id = $1", id).fetch_all(&data.db),
    sqlx::query_scalar!("select count(*) from net_devices nd JOIN net_cred nc on nd.cred_id = nc.id WHERE nc.account_id = $1", id).fetch_one(&data.db)
    );

    match q {
        Ok((user, groups, contract, transactions, fetch_realms, device_count)) => {
            Ok(Json(PersonOverviewModel {
                account: user,
                groups,
                contract,
                transactions,
                network: PersonOverviewNetworkModel {
                    realms: fetch_realms,
                    devices: device_count.unwrap_or(0),
                },
            }))
        }
        Err(_) => Err(VialoError::NotFound()),
    }
}

#[utoipa::path(post, path = "/people", request_body=CreateUserSchema, responses((status = 201, description = "Created", body=CreatePersonResponse)))]
pub async fn add_person(
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<CreateUserSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?; // begin transaction

    let created_user = sqlx::query!(
        r#"INSERT INTO accounts_people (label, email, membership_end, full_name, auth_id) VALUES ($1, $2, $3, $4, $5) RETURNING id, full_name"#,
        body.label, body.email, body.membership_end as PgDate, body.full_name, body.auth_id
    )
    .fetch_one(trans.deref_mut())
    .await?;

    if body.contract.is_some() {
        let contract = body.contract.unwrap();

        let find_room = sqlx::query!("SELECT id FROM res_rooms WHERE label = $1", contract.room)
            .fetch_one(&data.db)
            .await;

        if find_room.is_err() {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "unknown_room".into(),
            ));
        }

        sqlx::query!(
                    "INSERT INTO res_leases (tenant_id, lessor_id, room_id, begins, ends) VALUES ($1, $2, $3, $4, $5)",
                    created_user.id,
                    contract.lessor_id,
                    find_room.unwrap().id,
                    contract.begins,
                    contract.ends
                )
                .execute(trans.deref_mut())
                .await?;
    }

    let mut response = json!({"id":  created_user.id, "full_name": created_user.full_name});

    if body.auto_setup_network.unwrap_or(false) {
        // Find a free realm which has an IPv4 NAT IP
        let assigned_realm = sqlx::query!(
            r#"WITH inserted AS (
                INSERT INTO net_realm_assignments (realm_id, account_id)
                SELECT id as realm_id, $1 as account_id
                FROM net_realms nr
                LEFT JOIN net_realm_assignments nra ON nr.id = nra.realm_id
                WHERE nra.realm_id IS NULL AND nr.ipv4_nat IS NOT NULL
                LIMIT 1
                RETURNING realm_id
            )
            SELECT
                inserted.realm_id AS id,
                nr.ipv4_nat
            FROM inserted
            JOIN net_realms nr ON nr.id = inserted.realm_id;"#,
            created_user.id
        )
        .fetch_one(trans.deref_mut())
        .await?;

        response.as_object_mut().unwrap().insert(
            "network".into(),
            json!({"realm": {"id": assigned_realm.id, "ipv4_nat": assigned_realm.ipv4_nat}}),
        );
    }

    #[cfg(feature = "printer")]
    if body.auto_setup_printer.unwrap_or(false) {
        let username = Alphanumeric.sample_string(&mut rng(), 8);
        let password = Alphanumeric.sample_string(&mut rng(), 16);

        printer::add_task(
            trans.deref_mut(),
            JobData::CreateAccount {
                account_id: created_user.id,
                username: username.clone(),
                password: encryption::encrypt(&password)?,
            },
        )
        .await?;

        response.as_object_mut().unwrap().insert(
            "printer".into(),
            json!({"username": username, "password": password}),
        );
    }

    trans.commit().await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(put, path = "/people/{id}", request_body=CreateUserSchema, responses((status = 204, description = "Updated")))] // no body
pub async fn put_person(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<CreateUserSchema>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    let updated_user = sqlx::query_as!(AccountModel,
        r#"UPDATE accounts_people SET label = $1, email = $2, membership_end = $3, full_name = $4 WHERE id = $5 RETURNING id,
               label,
             auth_id,
              email,
             membership_start,
               membership_end as "membership_end: PgDate",
                full_name,
                manually_suspended,
               credit_balance"#,
        body.label, body.email, body.membership_end as PgDate, body.full_name, id
    )
    .fetch_one(trans.deref_mut())
    .await?;

    if let Some(contract) = body.contract {
        let find_room = sqlx::query!("SELECT id FROM res_rooms WHERE label = $1", contract.room)
            .fetch_one(&data.db)
            .await;

        if find_room.is_err() {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "unknown_room".into(),
            ));
        }

        sqlx::query!(
                    "INSERT INTO res_leases (tenant_id, lessor_id, room_id, begins, ends) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (tenant_id) DO UPDATE SET (lessor_id, room_id, begins, ends) = ($2, $3, $4, $5)",
                    updated_user.id,
                    contract.lessor_id,
                    find_room.unwrap().id,
                    contract.begins,
                    contract.ends
                )
                .execute(trans.deref_mut())
                .await?;

        trans.commit().await?;
        Ok(StatusCode::NO_CONTENT)
    } else {
        sqlx::query!(
            "DELETE FROM res_leases WHERE tenant_id = $1",
            updated_user.id
        )
        .execute(trans.deref_mut())
        .await?;

        trans.commit().await?;
        Ok(StatusCode::NO_CONTENT)
    }
}

#[utoipa::path(delete, path = "/people/{id}", responses((status = 204, description = "Deleted")))]
pub async fn delete_person(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::AccountManager, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    #[cfg(feature = "printer")]
    if let Some(printer_id) = sqlx::query_scalar!(
        "SELECT printer_id FROM subsystem_printer_context WHERE id = $1",
        id
    )
    .fetch_optional(&mut *trans)
    .await?
    {
        printer::add_task(&mut *trans, JobData::DeleteAccount { printer_id }).await?;
    }

    sqlx::query!("DELETE FROM accounts WHERE id = $1", id)
        .execute(&mut *trans)
        .await?;

    helpers::people::delete_identity(id, &mut trans)
        .await
        .map_err(VialoError::Anyhow)?;

    trans.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sets credits to 0 if it was null.
#[utoipa::path(post, path = "/people/{id}/credits", responses((status = 204, description = "Enabled")))]
pub async fn enable_credits(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    check_app_role(user.clone(), AppRole::CreditManager, &data.db).await?;

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let result = query!(
        "UPDATE accounts_people SET credit_balance = 0 WHERE id = $1 AND credit_balance IS NULL",
        id
    )
    .execute(&mut *conn)
    .await?;

    if result.rows_affected() == 0 {
        return Err(VialoError::NotFound());
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
pub struct CreditBalanceResponse {
    pub credit_balance: i32,
}

#[utoipa::path(get, path = "/people/{id}/credits", responses((status = 200, description = "OK", body=CreditBalanceResponse)))]
pub async fn get_credits(
    Path(id): Path<Uuid>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    if user.id != id {
        check_app_role(user.clone(), AppRole::CreditManager, &data.db).await?;
    }
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let record = sqlx::query_scalar!(
        "SELECT credit_balance FROM accounts_people WHERE id = $1",
        id
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok((
        StatusCode::OK,
        Json(CreditBalanceResponse {
            credit_balance: record,
        }),
    ))
}
