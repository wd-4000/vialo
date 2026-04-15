use serde::de::{self, Deserializer, Unexpected};
use serde::{Deserialize, Serialize};
use serde_repr::*;

#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug)]
#[repr(u8)]
pub enum Action {
    Off = 0,
    On = 1,
    ShortOff = 2,
    ShortOn = 3,
    Toggle = 4,
    NoChange = 5,
    Ignore = 6,
}

fn bool_from_int<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match u8::deserialize(deserializer)? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(de::Error::invalid_value(
            Unexpected::Unsigned(other as u64),
            &"Netio returned something other than 0 or 1 for a boolean",
        )),
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Output {
    #[serde(rename = "ID")]
    pub id: i32,
    pub name: String,
    #[serde(deserialize_with = "bool_from_int")]
    pub state: bool,
    pub action: Action,
    pub delay: i32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Agent {
    pub model: String,
    pub device_name: String,
    #[serde(rename = "MAC")]
    pub mac: String,
    pub serial_number: String,
    #[serde(rename = "JSONVer")]
    pub json_ver: String,
    pub time: String,
    pub uptime: u64,
    pub version: String,
    #[serde(rename = "OemID")]
    pub oem_id: i32,
    #[serde(rename = "VendorID")]
    pub vendor_id: i32,
    pub num_outputs: i32,
    pub num_inputs: i32,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct NetioGet {
    pub agent: Agent,
    pub outputs: Vec<Output>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct OutputPost {
    #[serde(rename = "ID")]
    pub id: i32,
    pub action: Action,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct NetioPost {
    pub outputs: Vec<OutputPost>,
}
