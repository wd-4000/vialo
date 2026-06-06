use super::models::NetAuth;
use crate::http::network::mac::MacAddressWrapper;
use serde::{Deserialize, Serialize};
use sqlx::types::ipnetwork;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Deserialize, Debug)]
pub struct ParamOptions {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CreateDeviceSchema {
    pub label: String,
    pub mac: MacAddressWrapper,
    pub cred_id: Uuid,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UpdateDeviceSchema {
    pub label: Option<String>,
    pub mac: Option<MacAddressWrapper>,
    pub cred_id: Option<Uuid>,
}
#[derive(Debug, Serialize, Deserialize)]
struct Node {
    node_type: String,
}

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct NetworkFilterOptions {
    pub search: Option<String>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct RealmFilterOptions {
    pub search: Option<String>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub account_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema)]
pub struct SetDefaultRealmSchema {
    pub realm_id: Option<Uuid>,
    pub account_id: Uuid,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateRealmSchema {
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_subnet: Option<ipnetwork::IpNetwork>,
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv6_prefix: Option<ipnetwork::IpNetwork>,
    #[schema(value_type = Option<String>, format = Ipv4)]
    pub ipv4_nat: Option<ipnetwork::IpNetwork>,
    pub vlan: Option<i32>,
}

#[derive(Deserialize, ToSchema)]
pub struct PostOrPutNetworkSchema {
    pub label: String,
    pub icon: Option<String>,
    pub wired: bool,
    pub multi_device: bool,
    pub auto_add_on_auth: bool,
    pub auto_add_via_dhcp: bool,
    pub auth: Option<NetAuth>,
    pub gen_username: Option<i32>,
    pub gen_password: Option<i32>,
    pub active: bool,
    pub ssid: Option<String>,
    pub tls_trust: Option<String>,
    pub outer_identity: Option<String>,
}
