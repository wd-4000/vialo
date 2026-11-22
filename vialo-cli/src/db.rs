//! `vialo db init`: Creates roles and databases on the local PostgreSQL
use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, bail};

/// (role, database). Roles match the units' DynamicUser names, so peer auth
/// lets each service in as its own role without a password.
const DATABASES: [(&str, &str); 2] = [("vialo-api", "vialo"), ("vialo-kratos", "kratos")];

/// Runs `sql` through psql as the postgres superuser, returning unaligned output
fn psql(db: &str, sql: &str) -> anyhow::Result<String> {
    let mut child = Command::new("runuser")
        .args(["-u", "postgres", "--", "psql", "-X", "-q", "-At"])
        .args(["-v", "ON_ERROR_STOP=1", "-d", db])
        // postgres usually can't enter the caller's cwd (e.g. /root)
        .current_dir("/")
        // Hides "already exists, skipping" on re-runs
        .env("PGOPTIONS", "-c client_min_messages=warning")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed running runuser. Is util-linux installed?")?;
    child.stdin.take().unwrap().write_all(sql.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("psql on {db} failed");
    }
    Ok(String::from_utf8(out.stdout)?)
}

pub fn init() -> anyhow::Result<()> {
    let version: u32 = psql("postgres", "SHOW server_version_num")?
        .trim()
        .parse()?;
    if version < 180000 {
        bail!("vialo needs PostgreSQL 18 or later, found server_version_num {version}");
    }

    // CREATE DATABASE can't go in a DO block, hence \gexec
    let mut sql = String::new();
    for (role, db) in DATABASES {
        sql += &format!(
            "SELECT 'CREATE ROLE \"{role}\" LOGIN' \
             WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{role}')\\gexec\n\
             SELECT 'CREATE DATABASE {db} OWNER \"{role}\"' \
             WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '{db}')\\gexec\n"
        );
    }
    // Grant DHCP tables to vialo-dhcp
    sql += "SELECT 'CREATE ROLE \"vialo-dhcp\" LOGIN' \
            WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'vialo-dhcp')\\gexec\n";
    psql("postgres", &sql)?;

    // Untrusted extension, so vialo-api's migration can't create it as the owner
    psql(
        "vialo",
        "CREATE EXTENSION IF NOT EXISTS pg_stat_statements;",
    )?;

    let mut misowned = false;
    for (role, db) in DATABASES {
        let owner = psql(
            "postgres",
            &format!("SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = '{db}'"),
        )?;
        let owner = owner.trim();
        if owner == role {
            println!("{db}: owned by {role}");
        } else {
            eprintln!("{db} already exists and is owned by {owner}, not {role}");
            misowned = true;
        }
    }
    if misowned {
        bail!("the services can't migrate databases they don't own");
    }
    Ok(())
}
