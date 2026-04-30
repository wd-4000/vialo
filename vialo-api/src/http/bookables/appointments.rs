use super::models::{BookableAssetStatus, BookableStatus};
use crate::helpers::PgDateTime;
use crate::http::util::models::{AccountEmbed, IdOrAllQuery};
use crate::http::util::{User, VialoError, grab_authd_conn_user};
use crate::permissions::{AppRole, check_app_role};
// use crate::ketoapi::subject::Ref;
// use crate::ketoapi::{self, CheckRequest, Subject};
use crate::AppState;
use axum::extract::Path;
use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::query_as;
use sqlx_conditional_queries::conditional_query_as;
use std::i64;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
// CREATE TABLE bookable_appointments (
//     id uuid PRIMARY KEY default gen_random_uuid (),
//     asset_id int NOT NULL REFERENCES bookable_assets (id),
//     transaction_id uuid REFERENCES credit_ledger (id),
//     account_id uuid NOT NULL REFERENCES accounts (id), -- No cascade here, we want to make sure appointments are refunded appropriately.
//     begins timestamptz NOT NULL,
//     ends timestamptz,
//     maintenance boolean
// );
//
#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct BookableAppointmentFilterOptions {
    pub lang: Option<Vec<String>>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub from: Option<NaiveDateTime>,
    pub account_id: Option<IdOrAllQuery>,
    pub to: Option<NaiveDateTime>,
    pub search: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct BookableAppointmentType {
    pub id: Uuid,
    pub asset_id: i32,
    pub transaction_id: Option<Uuid>,
    pub account: Option<AccountEmbed>,
    pub begins: Option<NaiveDateTime>,
    pub ends: Option<PgDateTime>,
    #[schema(format = DateTime)]
    pub activated: Option<chrono::DateTime<chrono::Utc>>,
    pub maintenance: Option<bool>,
}

#[utoipa::path(post, path = "/bookables/appointments/{id}/activate", responses((status = 204, description = "Activated")))] // no body
pub async fn activate(
    Path(id): Path<Uuid>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    let bookable_record = query_as!(BookableAssetStatus, r#"update bookable_appointments set activated = NOW() FROM bookable_appointments bap JOIN bookable_assets ba ON bap.asset_id = ba.id WHERE bap.id = $1 AND bap.account_id = $2 RETURNING bap.asset_id as "id!", ba.asset_type as "asset_type!", 'active'::bookable_status_type as "status!: BookableStatus",
        upper(bap.during) as begins,
        lower(bap.during) as "ends: PgDateTime";"#, id, user.id).fetch_one(&mut *conn).await?;

    let evil_data = data.clone();
    tokio::spawn(async move {
        evil_data
            .event_channel
            .broadcast(bookable_record.asset_type, bookable_record)
            .await;
    });

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/bookables/appointments", params(
    BookableAppointmentFilterOptions
), responses((status = 200, description = "OK", body=Vec<BookableAppointmentType>)))]
pub async fn list(
    Query(opts): Query<BookableAppointmentFilterOptions>,
    Extension(user): Extension<User>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);

    let _langs = opts
        .lang
        .unwrap_or(vec![String::from("en"), String::from("de")]);

    let offset = (opts.page.unwrap_or(1) - 1) * limit;
    // Listing all accounts' appointments requires BookableManager.
    if let Some(IdOrAllQuery::All) = &opts.account_id {
        check_app_role(user.clone(), AppRole::BookableManager, &data.db).await?;
    }

    let current_account_id = user.id;
    // Execute the query and handle the result
    let record = conditional_query_as!(
        BookableAppointmentType,
        r#"SELECT
            ba.id,
            ba.asset_id,
            ba.transaction_id,
            {#account_info}
            lower(ba.during) as begins,
            upper(ba.during) as "ends: PgDateTime",
            ba.activated,
            ba.maintenance
        FROM
            bookable_appointments ba LEFT JOIN accounts_people ap ON ba.account_id = ap.id LEFT JOIN account_groups ag ON ba.account_id = ag.id WHERE true {#account} {#from} {#to} ORDER BY during LIMIT {limit} OFFSET {offset}"#,
            #account_info = match(&opts.account_id){
                  Some(IdOrAllQuery::All) => r#"jsonb_build_object('id', ba.account_id, 'full_name', COALESCE(ap.full_name, ag.label), 'type', (CASE WHEN ap.id IS NOT NULL THEN 'person' ELSE 'group' END)) AS "account: AccountEmbed","#,
                  _ => r#"null AS "account: AccountEmbed","#
            },
            #account = match (&opts.account_id) {
                Some(IdOrAllQuery::All) => "",
                Some(IdOrAllQuery::Id(account)) => "AND account_id = {account}",
                None => "AND account_id = {current_account_id}",
            },
            #from = match (opts.from) {
                Some(from) => "AND ba.during @> {from}::timestamp ",
                None => ""
            },
            #to = match (opts.to) {
                Some(to) => "AND UPPER(ba.during) <= {to} ",
                None => ""
            }

    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}
