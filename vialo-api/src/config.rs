use serde::{Deserialize, Serialize};
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

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
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

#[derive(Deserialize)]
pub struct Config {
    pub proxy: Option<String>,
    pub public: PublicApiConfig,
    pub hooks: HooksApiConfig,
    pub auth: AuthConfig,
    pub org: OrgConfig,

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
    pub email_domain: String,
    pub org: OrgConfig,
}

impl From<&Config> for PublicConfig {
    fn from(config: &Config) -> Self {
        Self {
            org: config.org.clone(),

            #[cfg(feature = "email")]
            email_domain: config.email_domain(),
        }
    }
}
