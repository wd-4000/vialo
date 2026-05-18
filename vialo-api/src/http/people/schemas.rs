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

/*
{
  "first_name": "Name",
  "last_name": "Surname",
  "email": "bruh@bruh.com",
  "contract": {
    "room": "0909",
    "begins": "2025-02-27",
    "ends": "2025-02-26"
  },
  "label": "",
  "membership_until": "0222-02-22"
}
*/

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct ContractInfo {
    pub room: String,
    pub begins: chrono::NaiveDate,
    pub ends: chrono::NaiveDate,
    pub lessor: Option<AccountPersonEmbed>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct ContractInfoSchema {
    pub room: String,
    pub begins: chrono::NaiveDate,
    pub ends: chrono::NaiveDate,
    pub lessor_id: Option<Uuid>,
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreateUserSchema {
    pub full_name: String,
    #[serde_as(as = "NoneAsEmptyString")]
    #[schema(format = Email)]
    pub email: Option<String>,
    pub auth_id: Option<Uuid>,
    pub contract: Option<ContractInfoSchema>,
    pub label: Option<String>,
    pub membership_end: PgDate,
    pub auto_setup_network: Option<bool>,
    pub auto_setup_printer: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct CreateRoomSchema {
    pub label: String,
    pub capacity: i32,
    pub floor: Option<i32>,
}
