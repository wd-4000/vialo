use std::collections::HashMap;

use axum::{
    Json,
    body::to_bytes,
    extract::{FromRequest, Request},
    http::header,
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Acquire, PgConnection, PgPool, PgTransaction, Postgres, pool::PoolConnection};
use utoipa::IntoParams;
use uuid::Uuid;

pub mod middleware;
pub mod models;

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct ListOptions {
    pub page: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct SearchableListOptions {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub search: Option<String>,
}

#[derive(Debug)]
pub enum VialoError {
    Anyhow(anyhow::Error),
    AppError(StatusCode, String),
    AppErrorWithDetails(StatusCode, String, serde_json::Value),
    NotFound(),
    Forbidden(),
}

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for VialoError {
    fn into_response(self) -> Response {
        if let VialoError::AppErrorWithDetails(status, code, details) = self {
            return (
                status,
                Json(json!({"status":"error", "code":code, "details":details})),
            )
                .into_response();
        }
        let (status, message) = match self {
            VialoError::Anyhow(rejection) => {
                // This generally shouldn't happen, let's log it
                tracing::error!(%rejection);

                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error".to_owned(),
                )
            }
            VialoError::NotFound() => {
                // This error is caused by bad user input so don't log it
                (StatusCode::NOT_FOUND, "not_found".to_owned())
            }
            VialoError::Forbidden() => {
                // This error is caused by bad user input so don't log it
                (StatusCode::FORBIDDEN, "forbidden".to_owned())
            }
            VialoError::AppError(code, info) => {
                // This error is caused by bad user input so don't log it
                (code, info.to_owned())
            }
            VialoError::AppErrorWithDetails(..) => unreachable!(),
        };

        (status, Json(json!({"status":"error", "code":message}))).into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
//
//

impl From<anyhow::Error> for VialoError {
    fn from(err: anyhow::Error) -> Self {
        Self::Anyhow(err)
    }
}

impl From<sqlx::Error> for VialoError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => Self::NotFound(),
            _ => Self::Anyhow(err.into()),
        }
    }
}

#[derive(Clone)]
pub struct User {
    pub id: Uuid,
}

pub fn get_i18n_arg_arrays(
    vec_data: Option<HashMap<String, String>>,
) -> (Vec<String>, Vec<String>) {
    if let Some(i18n_map) = vec_data {
        let mut lang_strings = Vec::new();
        let mut content_strings = Vec::new();
        for (lang, content) in i18n_map {
            lang_strings.push(lang);
            content_strings.push(content);
        }
        (lang_strings, content_strings)
    } else {
        (vec![], vec![])
    }
}

pub async fn insert_i18n_strings(
    db: &mut PgConnection,
    vec_data: Vec<(&'static str, Option<HashMap<String, String>>)>,
) -> Result<HashMap<&'static str, i32>, sqlx::Error> {
    let i18n_fields = vec_data
        .into_iter()
        .filter_map(|(field, value)| value.map(|v| (field, v)))
        .collect::<HashMap<_, _>>();

    let mut processed_i18n_fields: HashMap<&str, i32> = HashMap::new();

    for (field_name, i18n_map) in i18n_fields {
        let mut lang_strings = Vec::new();
        let mut content_strings = Vec::new();
        for (lang, content) in i18n_map {
            lang_strings.push(lang);
            content_strings.push(content);
        }
        match sqlx::query!(
            "SELECT insert_i18n_strings($1, $2)",
            &lang_strings,
            &content_strings
        )
        .fetch_one(&mut *db)
        .await
        {
            Ok(device) => {
                processed_i18n_fields.insert(field_name, device.insert_i18n_strings.unwrap());
            }
            Err(e) => {
                return Err(e);
            }
        }
    }

    Ok(processed_i18n_fields)
}

/**
    Grabs a transaction. ``Err`` can be pumped directly out of the request.
*/
pub async fn grab_trans(
    conn: &mut PoolConnection<Postgres>,
) -> Result<PgTransaction<'_>, VialoError> {
    let trans = conn.begin().await?;
    Ok(trans)
}

/**
    Grabs a connection with the ``app.account_id`` attribute set to the provided value. ``Err`` can be pumped directly out of the request.
*/
pub async fn grab_authd_conn_user<T: ToString>(
    db: &PgPool,
    account_id: T,
) -> Result<PoolConnection<Postgres>, VialoError> {
    let mut conn = db.acquire().await?;

    sqlx::query!(
        "SELECT set_config('app.account_id', $1, false)",
        account_id.to_string()
    )
    .fetch_optional(&mut *conn)
    .await?;

    Ok(conn)
}

// We define our own `Json` extractor that customizes the error from `axum::Json`
pub struct JsonE<T>(pub T);

pub struct MaybeJsonE<T>(pub Option<T>);

impl<S, T> FromRequest<S> for MaybeJsonE<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = (StatusCode, axum::Json<Value>);

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match JsonE::<T>::from_request(req, state).await {
            Ok(JsonE(value)) => Ok(Self(Some(value))),
            Err(_) => Ok(Self(None)),
        }
    }
}

/// Parsed components of a serde_json error message for type/value mismatches.
struct ParsedSerdeError {
    received: Option<String>,
    expected: Option<String>,
}

/// Attempts to split serde_json error messages of the form
/// "invalid type: {received}, expected {expected}" (and "invalid value: …")
/// into their constituent parts.
fn parse_serde_error(msg: &str) -> ParsedSerdeError {
    for prefix in ["invalid type: ", "invalid value: "] {
        if let Some(rest) = msg.strip_prefix(prefix)
            && let Some(sep) = rest.find(", expected ")
        {
            return ParsedSerdeError {
                received: Some(rest[..sep].to_string()),
                expected: Some(rest[sep + ", expected ".len()..].to_string()),
            };
        }
    }
    ParsedSerdeError {
        received: None,
        expected: None,
    }
}

impl<S, T> FromRequest<S> for JsonE<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = (StatusCode, axum::Json<Value>);

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();

        // Verify Content-Type is application/json
        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !content_type.starts_with("application/json") {
            return Err((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                axum::Json(json!({
                    "status": "error",
                    "code": "invalid_content_type",
                    "message": "Expected Content-Type: application/json"
                })),
            ));
        }

        // Buffer the full request body
        let bytes = to_bytes(body, usize::MAX).await.map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({
                    "status": "error",
                    "code": "invalid_json",
                    "message": "Failed to read request body"
                })),
            )
        })?;

        // Deserialize with field-path tracking so we can pinpoint exactly which
        // field caused the error and strip the noisy "at line X column Y" suffix.
        let jd = &mut serde_json::Deserializer::from_slice(&bytes);
        serde_path_to_error::deserialize::<_, T>(jd)
            .map(Self)
            .map_err(|err| {
                let path = err.path().to_string();
                let inner = err.inner().to_string();

                // serde_json appends " at line N column M" — trim it for cleaner output
                let clean = inner
                    .find(" at line ")
                    .map(|i| inner[..i].to_string())
                    .unwrap_or(inner);

                // "." means the error is at the root level (e.g. syntax error or wrong root type)
                let parsed = parse_serde_error(&clean);

                let payload = {
                    let mut map = serde_json::Map::new();
                    map.insert("status".into(), json!("error"));
                    map.insert("code".into(), json!("invalid_json"));
                    if path == "." || path.is_empty() {
                        map.insert("message".into(), json!(clean));
                    } else {
                        map.insert(
                            "message".into(),
                            json!(format!("Invalid value for field '{}': {}", path, clean)),
                        );
                        map.insert("field".into(), json!(path));
                    }
                    if let Some(received) = parsed.received {
                        map.insert("received".into(), json!(received));
                    }
                    if let Some(expected) = parsed.expected {
                        map.insert("expected".into(), json!(expected));
                    }
                    Value::Object(map)
                };

                (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(payload))
            })
    }
}
