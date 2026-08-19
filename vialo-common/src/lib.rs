//! Startup and infrastructure mechanics shared by the vialo binaries.
//!
//! What belongs here: things every binary does before it starts serving —
//! finding and parsing the config file, and (should it ever move) connection
//! setup. What does not: domain types and API schema. In particular, nothing
//! deriving `utoipa::ToSchema` can live here — `utoipauto` discovers schemas by
//! scanning `vialo-api`'s own `src/http` and `src/hooks` trees (see that
//! crate's `build.rs`), so a type moved out of it silently disappears from the
//! OpenAPI document.
//!
//! Each binary defines its own `Config` struct and deserializes only the
//! sections it cares about; serde ignores the rest, so `vialo.toml` is one file
//! read by several processes that each see a different slice of it.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};

/// Config file name, searched for upward from the working directory.
pub const CONFIG_FILE: &str = "vialo.toml";

/// Skips search and uses this value instead
pub const CONFIG_ENV: &str = "VIALO_CONFIG";

/// Search the working directory and each of its parents for `filename`.
pub fn find_upward(filename: &str) -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(filename);
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Locate the config file: `$VIALO_CONFIG` if set, else the nearest `vialo.toml`
/// at or above the working directory.
pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_ENV) {
        let path = PathBuf::from(path);
        if !path.exists() {
            anyhow::bail!(
                "{CONFIG_ENV} is set to {}, which does not exist",
                path.display()
            );
        }
        return Ok(path);
    }

    find_upward(CONFIG_FILE).with_context(|| {
        format!(
            "could not find {CONFIG_FILE} in the working directory or any parent \
             (set {CONFIG_ENV} to an explicit path)"
        )
    })
}

/// Improved error message for toml parsing
pub fn load_from<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("error reading {}", path.display()))?;

    toml::from_str(&text).map_err(|e| {
        let location = e.span().map_or(String::new(), |span| {
            let line = text[..span.start].matches('\n').count() + 1;
            format!(" around line {line}")
        });
        anyhow::anyhow!("{} in {}{}", e.message(), path.display(), location)
    })
}

pub fn load<T: DeserializeOwned>() -> Result<T> {
    load_from(&config_path()?)
}
