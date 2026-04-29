use super::models::{HistoryEntryModel, Subsystem, TgOp};
use super::schemas::HistoryFilterOptions;
use std::collections::HashMap;

use crate::AppState;
use crate::http::util::VialoError;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use axum_extra::extract::Query;
use sqlx_conditional_queries::conditional_query_as;
use std::sync::Arc;

#[utoipa::path(get, path = "/history/schema", responses((status = 200, description = "OK", body = HashMap<String, Vec<String>>)))]
pub async fn list_schema(
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    // let search = opts.search.unwrap_or("".to_string());
    // This might be red in rust-analyzer. Ignore this.
    let record = sqlx::query!(
        r#"SELECT json_object_agg(table_name, columns) AS res
        FROM (SELECT array_agg(column_name)::text[] as columns, table_name
        FROM information_schema.columns WHERE table_schema = 'public' GROUP BY table_name);"#,
    )
    .fetch_one(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record.res)))
}

#[utoipa::path(get, path = "/history", params(HistoryFilterOptions), responses((status = 200, description = "OK", body=Vec<HistoryEntryModel>)))]
pub async fn list_history(
    Query(opts): Query<HistoryFilterOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let limit = opts.limit.unwrap_or(10);
    // let search = opts.search.unwrap_or("".to_string());
    let offset = (opts.page.unwrap_or(1) - 1) * limit;
    // This might be red in rust-analyzer. Ignore this.
    let record = conditional_query_as!(
        HistoryEntryModel,
        r#"WITH i18n_cols AS (
            SELECT tc.table_name, array_agg(kcu.column_name::text) as cols
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
            JOIN information_schema.constraint_column_usage ccu
              ON ccu.constraint_name = tc.constraint_name
              AND ccu.table_schema = tc.table_schema
            WHERE tc.constraint_type = 'FOREIGN KEY'
              AND ccu.table_name = 'i18n_index'
            GROUP BY tc.table_name
        )
        SELECT tl.id,
        tl.table_name,
        record_id,
        record_uuid,
        a2.full_name as object_name,
        operation_type as "operation_type: TgOp",
        created_at,
        delete_on,
        CASE WHEN tl.caused_by_account IS NOT NULL THEN jsonb_build_object('id', tl.caused_by_account, 'full_name', a.full_name) ELSE NULL END AS caused_by_account,
        caused_by_subsystem as "caused_by_subsystem: Subsystem",
        resolve_i18n_in_diff(diff, COALESCE(i18n_cols.cols, ARRAY[]::text[])) as diff
        FROM table_log tl
        LEFT JOIN accounts_people a ON tl.caused_by_account = a.id
        LEFT JOIN accounts_people a2 ON tl.table_name = 'accounts_people' AND a2.id = tl.record_uuid
        LEFT JOIN i18n_cols ON i18n_cols.table_name = tl.table_name
        {#wh}
        ORDER BY created_at DESC LIMIT {limit} OFFSET {offset}"#,
        #wh = match (opts.table){
            Some(tables) => "WHERE tl.table_name = ANY({tables:Vec<String>}) ",
            _ => r#"WHERE NOT (tl.table_name = ANY(ARRAY['i18n_strings', 'i18n_index']))"#
        }
    )    .fetch_all(&data.db)
    .await?;

    Ok((StatusCode::OK, Json(record)))
}
