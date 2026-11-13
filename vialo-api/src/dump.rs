use anyhow::anyhow;
use chrono::Local;
use reqwest::Url;
use tokio::process::Command;

/** Drop query parameters libpq doesn't like from the connection URI. */
fn libpq_uri(uri: &str) -> Result<String, anyhow::Error> {
    let mut url = Url::parse(uri)?;
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k != "schema")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept);
    }
    Ok(url.into())
}

/** Take a dump.

Of the database.
*/
pub async fn dump(uri: &str) -> Result<String, anyhow::Error> {
    let uri = &libpq_uri(uri)?;
    let filename = Local::now()
        .format("vialo_%Y-%m-%d_%H-%M-%S.dump")
        .to_string();
    // database: uri, Format: custom, no password, file: path
    let output = Command::new("pg_dump")
        .args(["-d", uri, "-F", "c", "-w", "-f", &*filename])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!("{}", String::from_utf8_lossy(&output.stderr)));
    }

    Ok(filename)
}

/** Restore a dump */
pub async fn restore(uri: &str, path: &std::path::PathBuf) -> Result<(), anyhow::Error> {
    let uri = &libpq_uri(uri)?;
    // No -C: with it, -c only drops the database (impossible while connected to it), not the objects.
    // database: uri, clean objects, tolerate missing ones, no password, file
    let output = Command::new("pg_restore")
        .args([
            "-d",
            uri,
            "-c",
            "--if-exists",
            "-w",
            &path.display().to_string(),
        ])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!("{}", String::from_utf8_lossy(&output.stderr)));
    }

    Ok(())
}
