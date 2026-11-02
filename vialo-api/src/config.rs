use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use strum::AsRefStr;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct PublicApiConfig {
    pub cors_origins: Vec<String>,
    pub listen: String,
}

#[derive(Deserialize)]
pub struct HooksApiConfig {
    pub listen: String,
}

#[derive(Deserialize, Clone, Serialize, ToSchema)]
pub struct OrgConfig {
    pub name: String,
    pub domain: String,
    pub short_name: String,
    pub impressum: String,
}

#[cfg(feature = "email")]
#[derive(Deserialize)]
pub struct EmailApiUrlConfig {
    pub unsubscribe: String,
    pub post: String,
    pub board: String,
    pub preferences: String,
}

#[cfg(feature = "email")]
#[derive(Deserialize)]
pub struct EmailApiConfig {
    domain: Option<String>,
    pub url: EmailApiUrlConfig,
}

#[derive(Deserialize, AsRefStr)]
#[serde(rename_all = "snake_case", tag = "type")]
#[strum(serialize_all = "snake_case")]
pub enum AuthConfig {
    Mock {
        uuid: Uuid,
        email: String,
    },
    Kratos {
        frontend_url: String,
        admin_url: String,
    },
}

fn default_timezone() -> String {
    "Europe/Berlin".to_string()
}

#[derive(Deserialize, Clone, Serialize, ToSchema)]
pub struct UiConfig {
    pub copyright: String,
    pub appname_user: String,
    pub appname_admin: String,
    pub appname_user_long: String,
    pub impressum_url: String,
    pub privacy_policy_url: String,
}

/// Credential for a fixed device (e.g. the laundry kiosk) that reads bookable
/// data without being an account. It's in config right now but I'm thinking of
/// replacing it with a device table.
#[derive(Deserialize, Clone)]
pub struct KioskConfig {
    pub token: String,
    pub asset_types: Vec<i32>,
}

impl KioskConfig {
    pub fn matches(&self, token: &str) -> bool {
        self.token == token
    }

    pub fn allows(&self, asset_type_id: i32) -> bool {
        self.asset_types.contains(&asset_type_id)
    }
}

#[derive(Deserialize, Clone)]
pub struct BookablesConfig {
    pub kiosk: Option<KioskConfig>,
}

fn default_media_dir() -> PathBuf {
    "media".into()
}

fn default_max_image_mb() -> u64 {
    25
}

fn default_max_document_mb() -> u64 {
    50
}

fn default_quota_mb() -> u64 {
    1024
}

#[derive(Deserialize)]
pub struct MediaConfig {
    /// Root for uploads
    #[serde(default = "default_media_dir")]
    pub dir: PathBuf,
    #[serde(default = "default_max_image_mb")]
    pub max_image_mb: u64,
    #[serde(default = "default_max_document_mb")]
    pub max_document_mb: u64,
    /// Total stored per user
    #[serde(default = "default_quota_mb")]
    pub quota_mb: u64,
    /// Encoder threads. Defaults to all cores but one
    pub encode_threads: Option<usize>,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            dir: default_media_dir(),
            max_image_mb: default_max_image_mb(),
            max_document_mb: default_max_document_mb(),
            quota_mb: default_quota_mb(),
            encode_threads: None,
        }
    }
}

#[cfg(feature = "migrate")]
#[derive(Deserialize)]
pub struct DevConfig {
    /// Rewrite `_sqlx_migrations` checksums that no longer match the migration
    #[serde(default)]
    pub repair_migration_checksums: bool,
}

#[derive(Deserialize)]
pub struct Config {
    pub proxy: Option<String>,
    /// IANA timezone name (e.g. "Europe/Berlin") the venue operates in. Bookable
    /// schedules are wall-clock times interpreted in this zone, so every DB
    /// session's `TimeZone` is set to it.
    #[serde(default = "default_timezone")]
    pub timezone: String,
    pub public: PublicApiConfig,
    pub hooks: HooksApiConfig,
    pub auth: AuthConfig,
    pub org: OrgConfig,
    pub ui: UiConfig,
    pub bookables: Option<BookablesConfig>,
    #[serde(default)]
    pub media: MediaConfig,

    #[cfg(feature = "migrate")]
    pub dev: Option<DevConfig>,

    #[cfg(feature = "email")]
    pub email: EmailApiConfig,
}

impl Config {
    #[cfg(feature = "email")]
    pub fn email_domain(&self) -> String {
        self.email
            .domain
            .as_deref()
            .unwrap_or(&self.org.domain)
            .to_string()
    }
}

#[derive(Serialize, ToSchema)]
pub struct PublicConfig {
    pub auth: String,
    pub timezone: String,
    pub org: OrgConfig,
    pub appname_user: String,
    pub appname_admin: String,
    pub appname_user_long: String,
    pub copyright: String,
    pub impressum_url: String,
    pub privacy_policy_url: String,
    #[cfg(feature = "email")]
    pub email_domain: String,
}

impl From<&Config> for PublicConfig {
    fn from(config: &Config) -> Self {
        Self {
            auth: config.auth.as_ref().to_string(),
            timezone: config.timezone.clone(),
            org: config.org.clone(),
            appname_user: config.ui.appname_user.clone(),
            appname_admin: config.ui.appname_admin.clone(),
            appname_user_long: config.ui.appname_user_long.clone(),
            copyright: config.ui.copyright.clone(),
            impressum_url: config.ui.impressum_url.clone(),
            privacy_policy_url: config.ui.privacy_policy_url.clone(),

            #[cfg(feature = "email")]
            email_domain: config.email_domain(),
        }
    }
}
