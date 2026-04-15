use std::error::Error;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::{
    Encode,
    encode::IsNull,
    postgres::{PgArgumentBuffer, PgTypeInfo, types::Oid},
};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum PgDateTime {
    Infinity,
    NegativeInfinity,
    DateTime(NaiveDateTime),
}

impl sqlx::Type<sqlx::Postgres> for PgDateTime {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <NaiveDateTime as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

impl sqlx::postgres::PgHasArrayType for PgDateTime {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        //PgType::TimestamptzArray
        PgTypeInfo::with_oid(Oid(1182))
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PgDateTime {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        match value.format() {
            sqlx::postgres::PgValueFormat::Text => match value.as_str()? {
                "infinity" => Ok(PgDateTime::Infinity),
                "-infinity" => Ok(PgDateTime::NegativeInfinity),
                _ => NaiveDateTime::decode(value).map(PgDateTime::DateTime),
            },
            sqlx::postgres::PgValueFormat::Binary => {
                let bytes = value.as_bytes()?;
                match bytes {
                    b"\x7F\xFF\xFF\xFF" | b"\x7F\xFF\xFF\xFF\xFF\xFF\xFF\xFF" => {
                        Ok(PgDateTime::Infinity)
                    }
                    b"\x80\x00\x00\x00" | b"\x80\x00\x00\x00\x00\x00\x00\x00" => {
                        Ok(PgDateTime::NegativeInfinity)
                    }
                    _ => NaiveDateTime::decode(value).map(PgDateTime::DateTime),
                }
            }
        }
    }
}

impl<'r> Encode<'r, sqlx::Postgres> for PgDateTime {
    #[allow(unused)]
    fn encode_by_ref(
        &self,
        buf: &mut PgArgumentBuffer,
    ) -> Result<IsNull, Box<dyn Error + Send + Sync + 'static>> {
        match self {
            PgDateTime::Infinity => {
                buf.extend(b"\x7F\xFF\xFF\xFF\xFF\xFF\xFF\xFF");
            }
            PgDateTime::NegativeInfinity => {
                buf.extend(b"\x80\x00\x00\x00\x00\x00\x00\x00");
            }
            PgDateTime::DateTime(naive_date) => {
                // Delegate the encoding to the NaiveDateTime type and handle potential errors
                <NaiveDateTime as Encode<sqlx::Postgres>>::encode_by_ref(naive_date, buf)?;
            }
        }
        Ok(IsNull::No)
    }
}
