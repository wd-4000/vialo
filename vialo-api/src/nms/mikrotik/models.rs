use anyhow::Result;
use mac_address::MacAddress;
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, StringWithSeparator, formats::CommaSeparator, serde_as};
use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    ops::RangeInclusive,
    str::FromStr,
};

#[derive(Debug, Clone)]
pub struct VlanRange(pub RangeInclusive<u16>);

impl FromStr for VlanRange {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let (start, end) = s.split_once('-').unwrap_or((s, s));
        Ok(Self(start.trim().parse()?..=end.trim().parse()?))
    }
}

impl fmt::Display for VlanRange {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match (self.0.start(), self.0.end()) {
            (s, e) if s == e => write!(f, "{s}"),
            (s, e) => write!(f, "{s}-{e}"),
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(default)]
pub struct BridgeVlan {
    pub bridge: String,
    #[serde_as(as = "StringWithSeparator::<CommaSeparator, VlanRange>")]
    pub vlan_ids: Vec<VlanRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde_as(as = "StringWithSeparator::<CommaSeparator, String>")]
    pub tagged: Vec<String>,
    #[serde_as(as = "StringWithSeparator::<CommaSeparator, String>")]
    pub untagged: Vec<String>,
    #[serde_as(as = "DisplayFromStr")]
    pub disabled: bool,
}

#[serde_as]
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(default)]
pub struct Neighbor {
    #[serde(rename = ".id")]
    pub id: String,
    pub address: Option<IpAddr>,
    pub address4: Option<Ipv4Addr>,
    pub address6: Option<Ipv6Addr>,
    pub age: String,
    pub board: Option<String>,
    pub discovered_by: String,
    pub identity: Option<String>,
    #[serde_as(as = "StringWithSeparator::<CommaSeparator, String>")]
    pub interface: Vec<String>,
    pub interface_name: String,
    pub platform: Option<String>,
    pub software_id: Option<String>,
    #[serde_as(as = "DisplayFromStr")]
    pub ipv6: bool,
    pub mac_address: MacAddress,
    #[serde_as(as = "StringWithSeparator::<CommaSeparator, String>")]
    pub system_caps: Vec<String>,
    #[serde_as(as = "StringWithSeparator::<CommaSeparator, String>")]
    pub system_caps_enabled: Vec<String>,
    pub system_description: Option<String>,
    pub unpack: String,
    pub version: Option<String>,
}

#[serde_as]
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(default)]
pub struct MultiPassphraseGroupPassphrase {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde_as(as = "DisplayFromStr")]
    pub disabled: bool,
    pub group: String,
    pub passphrase: String,
    #[serde_as(as = "Option<DisplayFromStr>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vlan_id: Option<i32>,
}

#[serde_as]
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
#[serde(rename_all = "kebab-case")]
pub struct MultiPassphraseGroupPassphraseGet {
    #[serde(rename = ".id")]
    pub id: String,
    #[serde(flatten)]
    pub data: MultiPassphraseGroupPassphrase,
    #[serde_as(as = "DisplayFromStr")]
    pub expired: bool,
}
