//! Building blocks for importing data from legacy systems.
//!
//! Instance-specific importers read their sources and write exclusively through
//! [`Importer`], which keeps everything in one transaction attributed to the
//! `import` subsystem and records where each row came from in `import_refs`.

use std::{collections::BTreeMap, fmt::Display, str::FromStr};

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};

use crate::http::util::VialoError;

pub mod bookables;
pub mod kratos;
pub mod ledger;
pub mod network;
pub mod people;

/// API inserts report `VialoError`, which isn't a std error
pub(crate) fn api_error(e: VialoError) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}

/// A record in a legacy source, e.g. `Ref::new("credits", "users", 42)`.
#[derive(Clone, Debug, Serialize)]
pub struct Ref {
    pub source: &'static str,
    pub kind: &'static str,
    pub key: String,
}

impl Ref {
    pub fn new(source: &'static str, kind: &'static str, key: impl Display) -> Self {
        Self {
            source,
            kind,
            key: key.to_string(),
        }
    }
}

impl Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}#{}", self.source, self.kind, self.key)
    }
}

#[derive(Debug, Serialize)]
pub struct Warning {
    pub source: Option<Ref>,
    pub message: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    /// Rows created, per kind of target
    pub created: BTreeMap<&'static str, usize>,
    /// Refs that already existed and were reused
    pub reused: BTreeMap<&'static str, usize>,
    pub warnings: Vec<Warning>,
}

pub struct Importer {
    trans: Transaction<'static, Postgres>,
    dry_run: bool,
    pub report: Report,
}

impl Importer {
    /// `timezone` must match the instance config, schedules are wall-clock times.
    pub async fn begin(pool: &PgPool, timezone: &str, dry_run: bool) -> Result<Self> {
        let mut trans = pool.begin().await?;

        sqlx::query!("SELECT set_config('app.subsystem', 'import', true)")
            .fetch_one(&mut *trans)
            .await?;
        sqlx::query!("SELECT set_config('TimeZone', $1, true)", timezone)
            .fetch_one(&mut *trans)
            .await?;

        Ok(Self {
            trans,
            dry_run,
            report: Report::default(),
        })
    }

    /// Commits, or rolls back on a dry run.
    pub async fn finish(self) -> Result<Report> {
        if self.dry_run {
            self.trans.rollback().await?;
        } else {
            self.trans.commit().await?;
        }
        Ok(self.report)
    }

    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    pub fn warn(&mut self, source: Option<&Ref>, message: impl Display) {
        let message = message.to_string();
        match source {
            Some(r) => tracing::warn!("{r}: {message}"),
            None => tracing::warn!("{message}"),
        }
        self.report.warnings.push(Warning {
            source: source.cloned(),
            message,
        });
    }

    pub(crate) fn conn(&mut self) -> &mut PgConnection {
        &mut self.trans
    }

    pub(crate) fn trans(&mut self) -> &mut Transaction<'static, Postgres> {
        &mut self.trans
    }

    /// What a ref was imported as, if it was.
    pub async fn lookup<T: FromStr>(&mut self, r: &Ref) -> Result<Option<T>>
    where
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        let target = sqlx::query_scalar!(
            "SELECT target FROM import_refs WHERE source = $1 AND kind = $2 AND source_key = $3",
            r.source,
            r.kind,
            r.key
        )
        .fetch_optional(&mut *self.trans)
        .await?;

        target
            .map(|t| t.parse().with_context(|| format!("{r}: bad target {t}")))
            .transpose()
    }

    /// Previously imported target of `r`, counted as reused.
    pub(crate) async fn existing<T: FromStr>(
        &mut self,
        r: &Ref,
        target_kind: &'static str,
    ) -> Result<Option<T>>
    where
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        let found = self.lookup(r).await?;
        if found.is_some() {
            *self.report.reused.entry(target_kind).or_default() += 1;
        }
        Ok(found)
    }

    /// Records another source record that maps onto an already imported `target`.
    pub async fn alias(&mut self, r: &Ref, target: impl Display) -> Result<()> {
        sqlx::query!(
            "INSERT INTO import_refs (source, kind, source_key, target) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
            r.source,
            r.kind,
            r.key,
            target.to_string()
        )
        .execute(&mut *self.trans)
        .await?;
        Ok(())
    }

    /// Records that `r` became `target`, counted as created.
    pub(crate) async fn remember(
        &mut self,
        r: Option<&Ref>,
        target_kind: &'static str,
        target: impl Display,
    ) -> Result<()> {
        *self.report.created.entry(target_kind).or_default() += 1;
        let Some(r) = r else {
            return Ok(());
        };

        sqlx::query!(
            "INSERT INTO import_refs (source, kind, source_key, target) VALUES ($1, $2, $3, $4)",
            r.source,
            r.kind,
            r.key,
            target.to_string()
        )
        .execute(&mut *self.trans)
        .await?;
        Ok(())
    }
}
