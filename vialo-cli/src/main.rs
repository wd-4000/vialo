//! Convenience binary to manage the database

use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::Shell;
use dotenv::dotenv;
use vialo_api::dump::{dump, restore};
mod db;
mod seed;

#[derive(Parser)]
#[command(name = "vialo", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Set up the local PostgreSQL
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Dump the database to a file
    Dump(DbUrl),
    /// Restore the database from a dump
    Restore {
        #[command(flatten)]
        db: DbUrl,
        #[arg(value_hint = ValueHint::FilePath)]
        path: PathBuf,
    },
    /// Seed rooms, realms and residents
    Seed(DbUrl),
    /// Print a shell completion script to stdout
    Completions { shell: Shell },
}

#[derive(Subcommand)]
enum DbCommand {
    /// Create the vialo-api and vialo-kratos roles and databases. Run as root. Safe to re-run
    Init,
}

#[derive(Args)]
struct DbUrl {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match dotenv() {
        Err(dotenv::Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {}
        res => _ = res.context("reading .env")?,
    }

    match Cli::parse().command {
        Command::Db {
            command: DbCommand::Init,
        } => db::init()?,
        Command::Dump(db) => {
            let filename = dump(&db.database_url).await.context("dumping")?;
            println!("Dumped DB to: {filename}");
        }
        Command::Restore { db, path } => {
            restore(&db.database_url, &path)
                .await
                .context("restoring")?;
            println!("Restored {}", path.display());
        }
        Command::Seed(db) => {
            let report = seed::seed(&db.database_url).await.context("seeding")?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_owned();
            clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
        }
    }
    Ok(())
}
