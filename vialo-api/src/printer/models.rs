use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::{error, fmt};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub enum PrinterRequestError {
    XMLError,
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

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde_as]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum JobData {
    CreateAccount {
        account_id: Uuid,
        username: String,
        // Manually handle encryption here to avoid ugly serialization
        #[serde_as(as = "serde_with::base64::Base64")]
        password: Vec<u8>,
    },
    FullSync {},
    Refresh {},
    UpdateAccountLimit {
        account_id: Uuid,
        color_limit: u16,
        bw_limit: u16,
    },
    DeleteAccount {
        printer_id: i32,
    },
}

#[derive(Debug, sqlx::FromRow, Deserialize, Serialize)]
pub struct JobModel {
    pub id: i32,
    pub data: sqlx::types::Json<JobData>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub status: JobStatus,
}
