use crate::helpers::encryption::{self as encryption, Encrypted};
use crate::http::util::models::AccountEmbed;
use crate::impl_jsonb_embed;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, types::ipnetwork};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, PartialOrd, sqlx::Type, Deserialize, Serialize, ToSchema)]
#[sqlx(type_name = "net_auth", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum NetAuth {
    UsernamePassword,
    Password,
}

#[derive(FromRow, Serialize, Deserialize, ToSchema)]
pub struct NetworkModel {
    pub id: i32,
    pub label: String,
    pub icon: Option<String>,
    pub multi_device: bool,
    pub auto_add_on_auth: bool,
    pub auto_add_via_dhcp: bool,
    pub auth: Option<NetAuth>,
    pub gen_username: Option<i32>,
    pub gen_password: Option<i32>,
    pub active: bool,
    pub wired: bool,
}
impl_jsonb_embed!(NetworkModel);

#[derive(FromRow, Serialize, ToSchema)]
pub struct RealmModel {
    pub id: Uuid,
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_subnet: Option<ipnetwork::IpNetwork>,
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv6_prefix: Option<ipnetwork::IpNetwork>,
    #[schema(value_type = Option<String>, format = Ipv6)]
    pub ipv4_nat: Option<ipnetwork::IpNetwork>,
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_dns: Option<ipnetwork::IpNetwork>,
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_router: Option<ipnetwork::IpNetwork>,

    pub vlan: Option<i32>,
}

#[derive(FromRow, Serialize, Deserialize, ToSchema)]
pub struct CredentialModelWithPassword {
    pub id: Uuid,
    pub username: Option<String>,
    #[serde(serialize_with = "encryption::serialize_exposed_opt")]
    #[schema(value_type = Option<String>)]
    pub password: Option<Encrypted<String>>,
    pub account_id: Uuid,
    pub network_id: i32,
    pub last_updated: Option<DateTime<Utc>>,
}

#[derive(FromRow, Serialize, Deserialize, ToSchema)]
pub struct CredentialModelWithAccountEmbed {
    pub id: Uuid,
    pub username: Option<String>,
    pub network_id: i32,
    pub last_updated: Option<DateTime<Utc>>,
    pub account: AccountEmbed, // TODO Make not optional
}

#[derive(FromRow, Serialize, Deserialize, ToSchema)]
pub struct CredentialModelWithPasswordAndAccountEmbed {
    pub id: Uuid,
    pub username: Option<String>,
    #[serde(serialize_with = "encryption::serialize_exposed_opt")]
    #[schema(value_type = Option<String>)]
    pub password: Option<Encrypted<String>>,
    pub network_id: i32,
    pub last_updated: Option<DateTime<Utc>>,
    pub account: AccountEmbed, // TODO Make not optional
}
