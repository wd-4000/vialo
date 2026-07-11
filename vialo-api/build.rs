// Generate absolute paths for utoipauto so proc-macro discovery works
// regardless of CWD (workspace root, crate root, rust-analyzer, etc.)
use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();

    let paths = format!(
        "{}/src/http, {}/src/hooks",
        manifest_dir, manifest_dir
    );

    let content = format!(
        r#"#[utoipauto(paths = "{paths}")]
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
"#,
    );

    let dest = Path::new(&out_dir).join("api_doc_gen.rs");
    fs::write(&dest, content).unwrap();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/http");
    println!("cargo:rerun-if-changed=src/hooks");
}
