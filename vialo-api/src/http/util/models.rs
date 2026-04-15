use serde::{Deserialize, Deserializer, de::Error};
use uuid::Uuid;

#[derive(Debug, PartialEq)]
pub enum IdOrAllQuery {
    Id(Uuid),
    All,
}

impl<'de> Deserialize<'de> for IdOrAllQuery {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?;

        Ok(match variant.as_str() {
            "all" => IdOrAllQuery::All,
            id => IdOrAllQuery::Id(id.parse::<Uuid>().map_err(Error::custom)?),
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum IdOrMeOrAllQuery {
    Id(Uuid),
    Me,
    All,
}

impl Default for IdOrMeOrAllQuery {
    fn default() -> Self {
        Self::All
    }
}

impl<'de> Deserialize<'de> for IdOrMeOrAllQuery {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?;

        Ok(match variant.as_str() {
            "all" => Self::All,
            "me" => Self::Me,
            id => Self::Id(id.parse::<Uuid>().map_err(Error::custom)?),
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum IdOrMeQuery {
    Id(Uuid),
    Me,
}

impl<'de> Deserialize<'de> for IdOrMeQuery {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant = String::deserialize(de)?;

        Ok(match variant.as_str() {
            "me" => Self::Me,
            id => Self::Id(id.parse::<Uuid>().map_err(Error::custom)?),
        })
    }
}

/** This is, like, an insane wrapper for an Option inside a uhhh Struct. So none = not present, some is either null or a value */
#[derive(Debug)]
pub enum PatchOption<T> {
    /** Field NOT THERE  */
    None,
    /** This might be null if you didn't understand */
    Some(Option<T>),
}

impl<T> Default for PatchOption<T> {
    fn default() -> Self {
        PatchOption::None
    }
}

impl<T> From<Option<T>> for PatchOption<T> {
    fn from(opt: Option<T>) -> PatchOption<T> {
        match opt {
            Some(v) => PatchOption::Some(Some(v)),
            None => PatchOption::Some(None),
        }
    }
}

impl<'de, T> Deserialize<'de> for PatchOption<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::deserialize(deserializer).map(Into::into)
    }
}
