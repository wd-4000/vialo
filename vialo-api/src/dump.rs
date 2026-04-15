use anyhow::anyhow;
use chrono::Local;
use tokio::process::Command;

/** Take a dump.

Of the database.
*/

pub async fn dump(uri: &str) -> Result<(), anyhow::Error> {
    let filename = Local::now()
        .format("vialo_%Y-%m-%d_%H-%M-%S.dump")
        .to_string();
    // database: uri, Format: custom, clean, Create, file: path
    let output = Command::new("pg_dump")
        .args(&["-d", uri, "-F", "c", "-c", "-C", "-f", &*filename])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!(
            "Something went wrong while dumping: {:?}",
            output.stderr
        ));
    }

    Ok(())
}
