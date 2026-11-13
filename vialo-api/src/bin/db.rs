//! Convenience binary to manage the database
//!
//! Usage: cargo run --bin db [dump|restore <filename>]

use dotenv::dotenv;
use vialo_api::dump::{dump, restore};

const USAGE: &str = "Usage: db [dump|restore <filename>]";

#[tokio::main]
async fn main() {
    match dotenv() {
        Err(dotenv::Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("Error reading .env file: {err}"),
        _ => {}
    }

    let mut args = std::env::args().skip(1);
    let operation = args.next().expect(USAGE);
    let path = args.next().map(std::path::PathBuf::from);

    let database_uri = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    match (operation.as_str(), path) {
        ("dump", _) => {
            let filename = dump(&database_uri)
                .await
                .unwrap_or_else(|err| panic!("Something went wrong while dumping: {err}"));
            println!("Dumped DB to: {filename}");
        }
        ("restore", Some(path)) => {
            restore(&database_uri, &path)
                .await
                .unwrap_or_else(|err| panic!("Something went wrong while restoring: {err}"));
            println!("Restored {}", path.display())
        }
        _ => panic!("{USAGE}"),
    }
}
