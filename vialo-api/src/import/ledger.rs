use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{Importer, Ref};
use crate::http::util::models::ProductType;

pub enum LedgerKind {
    /// Positive credits the account, negative debits it
    Transfer(i32),
    UnlimitedOn,
    UnlimitedOff,
}

pub struct LedgerImport {
    pub account: Uuid,
    pub kind: LedgerKind,
    pub product: Option<ProductType>,
    pub label: Option<String>,
    pub at: DateTime<Utc>,
    /// Debit this credits back, dropped with a warning if it doesn't fit
    pub refund_of: Option<Uuid>,
}

impl Importer {
    /// Inserts a ledger entry. Replay in chronological order.
    pub async fn ledger(&mut self, r: Option<&Ref>, e: LedgerImport) -> Result<Uuid> {
        if let Some(r) = r
            && let Some(id) = self.existing(r, "ledger").await?
        {
            return Ok(id);
        }

        let id = match e.kind {
            LedgerKind::Transfer(credits) => {
                let (from, to) = if credits < 0 {
                    (Some(e.account), None)
                } else {
                    (None, Some(e.account))
                };
                let mut product = e.product;
                let mut refund_of = e.refund_of;

                if let Some(target) = refund_of {
                    let original = sqlx::query!(
                        r#"SELECT credits - credits_refunded AS "refundable!", product AS "product: ProductType", from_account
                        FROM credit_ledger WHERE id = $1 AND entry_type = 'transfer'"#,
                        target
                    )
                    .fetch_optional(self.conn())
                    .await?;

                    match original {
                        Some(o)
                            if credits >= 0
                                && o.refundable >= credits
                                && o.from_account == Some(e.account) =>
                        {
                            product = product.or(o.product);
                        }
                        _ => {
                            self.warn(
                                r,
                                format!(
                                    "refund of {credits} doesn't fit {target}, imported unlinked"
                                ),
                            );
                            refund_of = None;
                        }
                    }
                }

                sqlx::query_scalar!(
                    "INSERT INTO credit_ledger (from_account, to_account, credits, product, label, created_at, refund_of)
                     VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
                    from,
                    to,
                    credits.abs(),
                    product as Option<ProductType>,
                    e.label,
                    e.at,
                    refund_of
                )
                .fetch_one(self.conn())
                .await?
            }
            // The pre-unlimited balance is kept in credits, so disabling can restore it.
            // A replay can run negative, which the ledger rejects there.
            LedgerKind::UnlimitedOn => {
                if let Some(balance) = self.balance(e.account).await?.filter(|b| *b < 0) {
                    self.warn(r, format!("account {}: balance {balance} when set to unlimited, kept as 0 to restore", e.account));
                }
                sqlx::query_scalar!(
                    "INSERT INTO credit_ledger (entry_type, from_account, credits, label, created_at)
                     VALUES ('unlimited_enabled', $1, (SELECT GREATEST(credit_balance, 0) FROM accounts_people WHERE id = $1), $2, $3)
                     RETURNING id",
                    e.account,
                    e.label,
                    e.at
                )
                .fetch_one(self.conn())
                .await?
            }
            LedgerKind::UnlimitedOff => {
                sqlx::query_scalar!(
                    "INSERT INTO credit_ledger (entry_type, to_account, label, created_at)
                     VALUES ('unlimited_disabled', $1, $2, $3) RETURNING id",
                    e.account,
                    e.label,
                    e.at
                )
                .fetch_one(self.conn())
                .await?
            }
        };

        self.remember(r, "ledger", id).await?;
        Ok(id)
    }

    /// Current balance, `None` = unlimited.
    pub async fn balance(&mut self, account: Uuid) -> Result<Option<i32>> {
        Ok(sqlx::query_scalar!(
            "SELECT credit_balance FROM accounts_people WHERE id = $1",
            account
        )
        .fetch_one(self.conn())
        .await?)
    }

    /// Transfers the difference to `target`. No-op while unlimited.
    pub async fn set_balance(
        &mut self,
        account: Uuid,
        target: i32,
        label: Option<String>,
        at: DateTime<Utc>,
    ) -> Result<Option<Uuid>> {
        let Some(balance) = self.balance(account).await? else {
            return Ok(None);
        };
        if balance == target {
            return Ok(None);
        }

        let entry = LedgerImport {
            account,
            kind: LedgerKind::Transfer(target - balance),
            product: None,
            label,
            at,
            refund_of: None,
        };
        Ok(Some(self.ledger(None, entry).await?))
    }

    /// Brings the account to the source's final state (`None` = unlimited), warning about any offset.
    pub async fn reconcile_balance(
        &mut self,
        r: &Ref,
        account: Uuid,
        target: Option<i32>,
        label: &str,
        at: DateTime<Utc>,
    ) -> Result<()> {
        let current = self.balance(account).await?;
        let entry = |kind| LedgerImport {
            account,
            kind,
            product: None,
            label: Some(label.to_string()),
            at,
            refund_of: None,
        };

        match (current, target) {
            (Some(_), None) => {
                self.warn(Some(r), "replay ended limited, source is unlimited");
                self.ledger(None, entry(LedgerKind::UnlimitedOn)).await?;
            }
            (None, Some(_)) => {
                self.warn(Some(r), "replay ended unlimited, source is limited");
                self.ledger(None, entry(LedgerKind::UnlimitedOff)).await?;
            }
            _ => {}
        }

        if let Some(target) = target {
            let balance = self.balance(account).await?.unwrap_or_default();
            if balance != target {
                self.warn(
                    Some(r),
                    format!(
                        "replayed balance {balance}, source {target}, offset {}",
                        target - balance
                    ),
                );
                self.set_balance(account, target, Some(label.to_string()), at)
                    .await?;
            }
        }
        Ok(())
    }
}
