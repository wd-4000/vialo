//! Prints the JSON Schema for `vialo.toml`
//!
//! Usage: cargo run --bin config-schema [output_path] (stdout if omitted)
//!
//! Shows enabled features only.

use schemars::JsonSchema;

#[derive(JsonSchema)]
#[schemars(title = "vialo.toml")]
#[allow(dead_code)]
struct VialoToml {
    #[schemars(flatten)]
    api: vialo_api::config::Config,
    #[schemars(flatten)]
    dhcp: vialo_common::dhcp::Config,
}

fn main() {
    let schema = schemars::schema_for!(VialoToml);
    let json = serde_json::to_string_pretty(&schema).expect("Failed to serialize schema");

    match std::env::args().nth(1) {
        Some(path) => std::fs::write(&path, json)
            .unwrap_or_else(|err| panic!("Failed to write schema file {path}: {err}")),
        None => println!("{json}"),
    }
}
