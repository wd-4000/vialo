use std::{fmt, str::FromStr};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};
use sqlx::{postgres::PgValueFormat, types::mac_address::MacAddress};

#[derive(Debug)]
pub struct MacAddressWrapper(mac_address::MacAddress);

impl Serialize for MacAddressWrapper {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string().to_ascii_lowercase())
    }
}

impl MacAddressWrapper {
    pub fn get_value(&self) -> mac_address::MacAddress {
        return self.0;
    }
}

struct MacAddrVisitor;

impl<'de> Visitor<'de> for MacAddrVisitor {
    type Value = MacAddressWrapper;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a mac address in string format")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if let Ok(res) = MacAddress::from_str(&value) {
            return Ok(MacAddressWrapper::from(res));
        } else {
            return Err(de::Error::custom("Invalid MAC"));
        }
    }
}

impl<'de> Deserialize<'de> for MacAddressWrapper {
    fn deserialize<D>(deserializer: D) -> Result<MacAddressWrapper, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(MacAddrVisitor)
    }
}

impl From<mac_address::MacAddress> for MacAddressWrapper {
    fn from(mac: mac_address::MacAddress) -> Self {
        MacAddressWrapper(mac)
    }
}

impl From<MacAddressWrapper> for mac_address::MacAddress {
    fn from(wrapper: MacAddressWrapper) -> Self {
        wrapper.0
    }
}

use sqlx::postgres::PgValueRef;
use sqlx::{Decode, Postgres, Type};

// Ridiculous impl to handle Option<MacAddressWrapper>
impl<'r> Decode<'r, Postgres> for MacAddressWrapper {
    fn decode(value: PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let bytes = match value.format() {
            PgValueFormat::Binary => value.as_bytes()?,
            PgValueFormat::Text => {
                return MacAddress::from_str(&value.as_str()?)
                    .map(MacAddressWrapper::from)
                    .map_err(|e| format!("Invalid MacAddr string: {}", e).into());
            }
        };

        if bytes.len() == 6 {
            return Ok(MacAddressWrapper(MacAddress::new(
                bytes.try_into().unwrap(),
            )));
        }

        Err(format!("Invalid MacAddr bytes: {:?}", bytes).into())
    }
}

impl Type<Postgres> for MacAddressWrapper {
    fn type_info() -> <Postgres as sqlx::Database>::TypeInfo {
        <String as Type<Postgres>>::type_info()
    }
}
