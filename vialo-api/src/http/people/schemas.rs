use crate::{
    helpers::PgDate,
    http::util::models::{AccountPersonEmbed, IdOrMeOrAllQuery},
};
use serde::{Deserialize, Serialize};
use serde_with::{NoneAsEmptyString, serde_as};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct TransactionFilterOptions {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    #[serde(default)]
    pub account_id: IdOrMeOrAllQuery,
}

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct UserFilterOptions {
    pub search: Option<String>,
    pub group: Option<Vec<Uuid>>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub resident: Option<bool>,
}

#[derive(Deserialize, Debug, Default, IntoParams)]
pub struct GroupFilterOptions {
    pub lang: Option<Vec<String>>,
    pub search: Option<String>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct ContractInfo {
    pub room: String,
    pub begins: chrono::NaiveDate,
    pub ends: chrono::NaiveDate,
    pub lessor: Option<AccountPersonEmbed>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct ContractInfoSchema {
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub room: String,
    pub begins: chrono::NaiveDate,
    pub ends: chrono::NaiveDate,
    pub lessor_id: Option<Uuid>,
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreateUserSchema {
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub full_name: String,
    #[serde_as(as = "NoneAsEmptyString")]
    #[schema(format = Email)]
    pub email: Option<String>,
    pub auth_id: Option<Uuid>,
    pub contract: Option<ContractInfoSchema>,
    #[serde(deserialize_with = "crate::helpers::limit_str_len_opt_255")]
    pub label: Option<String>,
    pub membership_end: PgDate,
    #[serde(default, deserialize_with = "crate::helpers::limit_str_len_opt_255")]
    pub phone: Option<String>,
    pub auto_setup_network: Option<bool>,
    pub enable_amenities_login: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreateRoomSchema {
    #[serde(deserialize_with = "crate::helpers::limit_str_len_255")]
    pub label: String,
    pub capacity: i32,
    pub floor: Option<i32>,
}
