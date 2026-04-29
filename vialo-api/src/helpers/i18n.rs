use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// A map of language codes to translated strings.
///
/// Represents internationalized content where keys are ISO language codes (e.g., "en", "de").
#[derive(Serialize, Deserialize, ToSchema)]
pub struct I18nMap(HashMap<String, String>);
