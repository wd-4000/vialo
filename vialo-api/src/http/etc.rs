// use crate::ketoapi::subject::Ref;
// use crate::ketoapi::{self, CheckRequest, Subject};
use crate::{AppState, http::util::VialoError};
use axum::http::header::{self};
use axum::{body::Body, extract::State, http::StatusCode, response::IntoResponse};
use sqlx::query;
use std::sync::Arc;

pub async fn list_mail_recipients(
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    // Execute the query and handle the result
    // match query!(r#"SELECT ARRAY_AGG(email) AS emails FROM (select ag.email from account_groups ag WHERE ag.email IS NOT NULL UNION SELECT slug from boards bd WHERE slug IS NOT NULL);"#,)
    let record = query!(
        r#"select ARRAY_AGG(ag.email) as emails from account_groups ag WHERE ag.email IS NOT NULL"#,
    )
    .fetch_one(&data.db)
    .await?;

    return Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Body::from(record.emails.map_or(String::from(""), |h| h.join("\n"))),
    ));
}
