use serde::{Deserialize, Serialize};
use std::{error, fmt};
use strum::AsRefStr;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub enum PrinterRequestError {
    XMLError,
    /// The referenced account does not exist on the device.
    AccountNotFound,
    /// Internal error
    Failure(Option<String>),
    Reqwest(#[serde(serialize_with = "serialize_reqwest_error")] reqwest::Error),
}

fn serialize_reqwest_error<S>(error: &reqwest::Error, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.collect_str(error)
}

impl fmt::Display for PrinterRequestError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PrinterRequestError::Failure(error_code) => {
                write!(
                    f,
                    "Request returned an error code: {}",
                    error_code.as_deref().unwrap_or("<unknown>")
                )
            }
            PrinterRequestError::XMLError => write!(f, "Printer returned unexpected XML."),
            PrinterRequestError::AccountNotFound => write!(f, "Printer account not found"),
            PrinterRequestError::Reqwest(..) => write!(f, "Reqwest error"),
        }
    }
}

impl error::Error for PrinterRequestError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match *self {
            // The cause is the underlying implementation error type. Is implicitly
            // cast to the trait object `&error::Error`. This works because the
            // underlying type already implements the `Error` trait.
            PrinterRequestError::Reqwest(ref e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize)]
#[sqlx(type_name = "job_status", rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Processing,
    Done,
    Error,
}

#[derive(Serialize, Deserialize, Debug, Clone, ToSchema)]
pub struct AccountInfo {
    #[schema(format = Email)]
    pub email: String,
    pub username: String,
    pub password: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, AsRefStr)]
#[serde(rename_all = "snake_case", tag = "type")]
#[strum(serialize_all = "snake_case")]
pub enum JobData {
    SyncAccount { account_id: Uuid },
    FullSync {},
    Refresh {},
    UpdateAccountLimit { account_id: Uuid },
    DeleteAccount { printer_id: i32 },
}

#[derive(Debug, sqlx::FromRow, Deserialize, Serialize)]
pub struct JobModel {
    pub id: i32,
    pub data: sqlx::types::Json<JobData>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub status: JobStatus,
}
