use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::AsRefStr;
use utoipa::ToSchema;
use uuid::Uuid;

/// Browser-facing API
#[derive(JsonSchema, Deserialize)]
pub struct PublicApiConfig {
    /// Frontend URLs that call this API
    pub cors_origins: Vec<String>,
    /// `/path` Unix socket, `@name` abstract socket, otherwise TCP `host:port`
    pub listen: String,
}

/// Webhooks (Kratos identity sync, RADIUS auth). No authentication!
#[derive(JsonSchema, Deserialize)]
pub struct HooksApiConfig {
    /// Same format as `public.listen`. Bind to `127.0.0.1` or a Unix socket
    pub listen: String,
}

#[derive(JsonSchema, Deserialize, Clone, Serialize, ToSchema)]
pub struct OrgConfig {
    /// Full organization name
    pub name: String,
    /// Domain for generated email addresses i.e. (`adminag@domain`)
    pub domain: String,
    /// Abbreviation used in the home group list
    pub short_name: String,
    /// URL of the legal notice / Impressum page
    pub impressum: String,
}

#[cfg(feature = "email")]
#[derive(JsonSchema, Deserialize)]
pub struct EmailApiUrlConfig {
    /// Base URL for one-click unsubscribe links
    pub unsubscribe: String,
    /// Base URL for links to individual posts
    pub post: String,
    /// Base URL for links to boards
    pub board: String,
    /// Base URL for email preference management
    pub preferences: String,
}

/// Transport comes from `EMAIL_URL` in the environment
#[cfg(feature = "email")]
#[derive(JsonSchema, Deserialize)]
pub struct EmailApiConfig {
    /// Defaults to `org.domain`
    domain: Option<String>,
    pub url: EmailApiUrlConfig,
}

/// Outbound URLs accept `unix:///path/to.sock` for HTTP over a Unix socket.
#[derive(JsonSchema, Deserialize, AsRefStr)]
#[serde(rename_all = "snake_case", tag = "type")]
#[strum(serialize_all = "snake_case")]
pub enum AuthConfig {
    /// Development only. The admin account is created on first start
    Mock {
        /// Always authenticates as this admin
        uuid: Uuid,
        /// Email of the auto-created admin account
        email: String,
    },
    Kratos {
        /// Kratos public API, for session validation and recovery emails
        frontend_url: String,
        /// Kratos admin API
        admin_url: String,
    },
}

fn default_timezone() -> String {
    "Europe/Berlin".to_string()
}

#[derive(JsonSchema, Deserialize, Clone, Serialize, ToSchema)]
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
#[derive(JsonSchema, Deserialize, Clone)]
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

#[derive(JsonSchema, Deserialize, Clone)]
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

#[derive(JsonSchema, Deserialize)]
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
#[derive(JsonSchema, Deserialize)]
pub struct DevConfig {
    /// Rewrite `_sqlx_migrations` checksums of migrations edited since they ran
    /// Does not apply the actual change, so change the DB yourself first
    #[serde(default)]
    pub repair_migration_checksums: bool,
}

#[derive(JsonSchema, Deserialize)]
pub struct Config {
    /// SOCKS5 URL for outbound HTTP (printer, bookable connectors, UniFi)
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
