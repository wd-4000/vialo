use std::error::Error;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::{
    Encode,
    encode::IsNull,
    postgres::{PgArgumentBuffer, PgTypeInfo, types::Oid},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PgDate {
    Infinity,
    NegativeInfinity,
    DateTime(NaiveDate),
}

impl sqlx::Type<sqlx::Postgres> for PgDate {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <NaiveDate as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

impl sqlx::postgres::PgHasArrayType for PgDate {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        //PgType::TimestamptzArray
        PgTypeInfo::with_oid(Oid(1182))
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PgDate {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        match value.format() {
            sqlx::postgres::PgValueFormat::Text => match value.as_str()? {
                "infinity" => Ok(PgDate::Infinity),
                "-infinity" => Ok(PgDate::NegativeInfinity),
                _ => NaiveDate::decode(value).map(PgDate::DateTime),
            },
            sqlx::postgres::PgValueFormat::Binary => {
                let bytes = value.as_bytes()?;
                match bytes {
                    b"\x7F\xFF\xFF\xFF" => Ok(PgDate::Infinity),
                    b"\x80\x00\x00\x00" => Ok(PgDate::NegativeInfinity),
                    _ => NaiveDate::decode(value).map(PgDate::DateTime),
                }
            }
        }
    }
}

impl<'r> Encode<'r, sqlx::Postgres> for PgDate {
    #[allow(unused)]
    fn encode_by_ref(
        &self,
        buf: &mut PgArgumentBuffer,
    ) -> Result<IsNull, Box<dyn Error + Send + Sync + 'static>> {
        match self {
            PgDate::Infinity => {
                buf.extend(b"\x7F\xFF\xFF\xFF");
            }
            PgDate::NegativeInfinity => {
                buf.extend(b"\x80\x00\x00\x00");
            }
            PgDate::DateTime(naive_date) => {
                // Delegate the encoding to the NaiveDate type and handle potential errors
                <NaiveDate as Encode<sqlx::Postgres>>::encode_by_ref(naive_date, buf)?;
            }
        }
        Ok(IsNull::No)
    }
}
