use super::permissions::{BookablePerm, require_asset_type_perm_by_asset};
use super::schemas::{NewSchemaInline, insert_schema};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;
use utoipa::ToSchema;

use crate::{
    AppState,
    http::util::{
        JsonE, ListOptions, User, VialoError, clamp_pagination, grab_authd_conn_user, grab_trans,
    },
};

#[derive(Deserialize, Serialize, ToSchema)]
pub struct BookableSchemaAssignment {
    pub begins: NaiveDate,
    pub label: Option<String>,
    pub schema_id: i32,
}

#[utoipa::path(get, path = "/bookables/{id}/schema_assignments", params(ListOptions), responses((status = 200, description = "OK", body=Vec<BookableSchemaAssignment>)))]
pub async fn list(
    Path(id): Path<i32>,
    Query(opts): Query<ListOptions>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm_by_asset(user.id, id, BookablePerm::Admin, &data.db).await?;
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let record = conditional_query_as!(
        BookableSchemaAssignment,
        r#"SELECT begins, schema_id, label
        FROM bookable_schema_assignments bsa
        JOIN bookable_schemas bs ON bs.id = bsa.schema_id
        WHERE asset_id = {id}
        ORDER BY begins
        LIMIT {limit} OFFSET {offset} "#
    )
    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}

#[derive(Deserialize, ToSchema)]
pub struct NewSchemaAssignment {
    pub begins: NaiveDate,
    /// Exactly one of schema_id / new_schema must be set.
    pub schema_id: Option<i32>,
    pub new_schema: Option<NewSchemaInline>,
}

#[derive(Deserialize, ToSchema)]
pub struct PostBookableSchemaAssignment {
    pub assignment: NewSchemaAssignment,
    pub existing_appointment_action: Option<ExistingAppointmentAction>,
}

enum SchemaInput {
    Existing(i32),
    New(NewSchemaInline),
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExistingAppointmentAction {
    Refund,
    Ignore,
}

#[utoipa::path(post, path = "/bookables/{id}/schema_assignments", request_body = PostBookableSchemaAssignment, responses((status = 204, description = "Created")))] //no body
pub async fn post(
    Path(id): Path<i32>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
    JsonE(body): JsonE<PostBookableSchemaAssignment>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm_by_asset(user.id, id, BookablePerm::Admin, &data.db).await?;

    let schema = match (body.assignment.schema_id, body.assignment.new_schema) {
        (Some(sid), None) => SchemaInput::Existing(sid),
        (None, Some(s)) => SchemaInput::New(s),
        _ => {
            return Err(VialoError::AppError(
                StatusCode::BAD_REQUEST,
                "exactly one of schema_id or new_schema must be set".into(),
            ));
        }
    };

    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;
    let mut trans = grab_trans(&mut conn).await?;

    // Existing appointments check
    let appointments_exist = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM bookable_appointments WHERE lower(during)::date >= $1 AND cancelled_at IS NULL AND activated IS NULL AND asset_id = $2) AS "exists!""#,
        body.assignment.begins,
        id
    )
    .fetch_one(&mut *trans)
    .await?;

    if appointments_exist {
        match body.existing_appointment_action {
            Some(ExistingAppointmentAction::Refund) => {
                // 100% refund for existing appointments
                sqlx::query!(
                    r#"WITH cancelled AS (
                        UPDATE bookable_appointments
                        SET cancelled_at = now(), cancellation_reason = 'admin'
                        WHERE lower(during)::date >= $1
                          AND cancelled_at IS NULL
                          AND activated IS NULL
                          AND asset_id = $2
                        RETURNING id, transaction_id
                    )
                    INSERT INTO credit_ledger (to_account, refund_of, credits)
                    SELECT cl.from_account, cancelled.transaction_id, cl.credits
                    FROM cancelled
                    JOIN credit_ledger cl ON cl.id = cancelled.transaction_id
                    WHERE cl.from_account IS NOT NULL AND cl.credits > 0"#,
                    body.assignment.begins,
                    id
                )
                .execute(&mut *trans)
                .await?;
            }
            Some(ExistingAppointmentAction::Ignore) => {
                // This is fine :)
            }
            None => {
                return Err(VialoError::AppError(
                    StatusCode::CONFLICT,
                    "appointments_exist".to_string(),
                ));
            }
        }
    }

    let schema_id = match schema {
        SchemaInput::Existing(sid) => sid,
        SchemaInput::New(s) => {
            let asset_type_id = sqlx::query_scalar!(
                "SELECT asset_type_id FROM bookable_assets WHERE id = $1",
                id
            )
            .fetch_one(&mut *trans)
            .await?;

            insert_schema(&mut trans, s, asset_type_id).await?
        }
    };

    sqlx::query!(
        "INSERT INTO bookable_schema_assignments (begins, schema_id, asset_id) VALUES ($1, $2, $3)",
        body.assignment.begins,
        schema_id,
        id
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/bookables/{id}/schema_assignments/{begins}", responses((status = 204, description = "Deleted")))] //no body
pub async fn delete(
    Path((id, begins)): Path<(i32, NaiveDate)>,
    State(data): State<Arc<AppState>>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, VialoError> {
    require_asset_type_perm_by_asset(user.id, id, BookablePerm::Admin, &data.db).await?;
    let mut conn = grab_authd_conn_user(&data.db, user.id).await?;

    sqlx::query!(
        "DELETE FROM bookable_schema_assignments WHERE begins = $1 AND asset_id = $2",
        begins,
        id
    )
    .execute(&mut *conn)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
