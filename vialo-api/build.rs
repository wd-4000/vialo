// Generate absolute paths for utoipauto so proc-macro discovery works
// regardless of CWD (workspace root, crate root, rust-analyzer, etc.)
use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();

    // The public API and the hooks API are served on separate listeners, so they
    // get separate documents — the UIs must never see the hooks contract.
    let public_paths = format!("{}/src/http", manifest_dir);
    let hooks_paths = format!("{}/src/hooks", manifest_dir);

    let content = format!(
        r#"#[utoipauto(paths = "{public_paths}")]
#[derive(OpenApi)]
#[openapi(
    modifiers(&FixTheseUglyTagsNow),
    info(
        title = "Vialo API",
        version = env!("CARGO_PKG_VERSION"),
        description = "OpenAPI specification generated from the Axum handlers."
    )
)]
pub struct ApiDoc;

pub fn openapi_doc() -> utoipa::openapi::OpenApi {{
    ApiDoc::openapi()
}}

#[utoipauto(paths = "{hooks_paths}")]
#[derive(OpenApi)]
#[openapi(
    modifiers(&FixTheseUglyTagsNow),
    info(
        title = "Vialo Hooks API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Internal endpoints called by FreeRADIUS and Kratos, served on a separate listener."
    )
)]
pub struct HooksApiDoc;

pub fn hooks_openapi_doc() -> utoipa::openapi::OpenApi {{
    HooksApiDoc::openapi()
}}
"#,
    );

    let dest = Path::new(&out_dir).join("api_doc_gen.rs");
    fs::write(&dest, content).unwrap();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/http");
    println!("cargo:rerun-if-changed=src/hooks");
}
