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
