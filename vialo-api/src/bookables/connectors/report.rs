use super::error::http_status;
use reqwest::StatusCode;
use serde::Serialize;
use utoipa::ToSchema;

/// What commissioning changed on a device. Kept when it then fails, as the device doesn't roll back.
#[derive(Serialize, Debug, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    AdminPasswordSet,
    RolesAdded,
}

#[derive(Serialize, Debug, ToSchema)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CommissionError {
    Unreachable { detail: String },
    LoginRejected { tries_left: Option<u64> },
    /// Too many rejected logins
    LoginBlocked { minutes: Option<u64> },
    OutputMissing,
    MissingPermissions { missing: Vec<i64> },
    Other { detail: String },
}

/// Why commissioning failed, and what it had changed on the device by then
#[derive(Serialize, Debug, ToSchema)]
pub struct CommissionFailure {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
    #[serde(flatten)]
    pub error: CommissionError,
}

/// What commissioning changed on the device
pub type CommissionResult = Result<Vec<Change>, CommissionFailure>;

impl CommissionFailure {
    /// Whether this differs from the failure stored last time, so it's news
    pub fn is_new(&self, stored: Option<&serde_json::Value>) -> bool {
        serde_json::to_value(self).ok().as_ref() != stored
    }
}

impl From<CommissionError> for CommissionFailure {
    fn from(error: CommissionError) -> Self {
        Self {
            output: None,
            changes: Vec::new(),
            error,
        }
    }
}

/// A failed device request: a 401 is a rejected login, no answer at all is unreachable
impl From<anyhow::Error> for CommissionError {
    fn from(e: anyhow::Error) -> Self {
        if http_status(&e) == Some(StatusCode::UNAUTHORIZED) {
            Self::LoginRejected { tries_left: None }
        } else if e.downcast_ref::<reqwest::Error>().is_some() {
            Self::Unreachable { detail: detail(&e) }
        } else {
            Self::Other { detail: detail(&e) }
        }
    }
}

/// The error for display and jsonb, which rejects NULs. Errors can carry whole response bodies, binary ones included.
fn detail(e: &anyhow::Error) -> String {
    const MAX: usize = 500;
    let full = format!("{e:#}");
    let mut chars = full.chars().filter(|c| !c.is_control() || *c == '\n');
    let mut detail: String = chars.by_ref().take(MAX).collect();
    if chars.next().is_some() {
        detail.push('…');
    }
    detail
}

impl From<anyhow::Error> for CommissionFailure {
    fn from(e: anyhow::Error) -> Self {
        CommissionError::from(e).into()
    }
}

/// What connector level commissioning found
#[derive(Debug)]
pub struct Discovery {
    pub info: Option<ConnectorInfo>,
    pub outputs: Vec<DiscoveredOutput>,
}

/// What a connector reports about itself
#[derive(Debug)]
pub struct ConnectorInfo {
    pub device_name: String,
    pub serial_number: String,
    pub mac: String,
}

#[derive(Debug)]
pub struct DiscoveredOutput {
    pub output_id: String,
    pub name: Option<String>,
}

