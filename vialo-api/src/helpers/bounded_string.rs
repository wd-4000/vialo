// serde gate functions that reject strings over a fixed length.
// Use with `#[serde(deserialize_with = "crate::helpers::limit_str_len_255")]`.
//
// Not a newtype because that'd mean implementing sqlx::Encode and annotating the type everywhere
use serde::{Deserialize, Deserializer};

macro_rules! limit_str_len {
    ($name:ident, $max:expr) => {
        pub fn $name<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
            let s = String::deserialize(d)?;
            if s.len() > $max {
                return Err(serde::de::Error::custom(format!(
                    "string too long ({} chars, max {})",
                    s.len(),
                    $max
                )));
            }
            Ok(s)
        }
    };
}

macro_rules! limit_str_len_opt {
    ($name:ident, $max:expr) => {
        pub fn $name<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
            match Option::<String>::deserialize(d)? {
                Some(s) if s.len() > $max => Err(serde::de::Error::custom(format!(
                    "string too long ({} chars, max {})",
                    s.len(),
                    $max
                ))),
                other => Ok(other),
            }
        }
    };
}

limit_str_len!(limit_str_len_255, 255);
limit_str_len!(limit_str_len_254, 254);
limit_str_len!(limit_str_len_128, 128);
limit_str_len!(limit_str_len_32, 32);

limit_str_len_opt!(limit_str_len_opt_255, 255);
limit_str_len_opt!(limit_str_len_opt_254, 254);
limit_str_len_opt!(limit_str_len_opt_128, 128);
limit_str_len_opt!(limit_str_len_opt_64, 64);
limit_str_len_opt!(limit_str_len_opt_32, 32);
limit_str_len_opt!(limit_str_len_opt_512, 512);
limit_str_len_opt!(limit_str_len_opt_8, 8);
