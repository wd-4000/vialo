use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use super::{Importer, Ref, api_error};
use crate::{
    bookables::{CancellationReason, MaintenanceRefund},
    helpers::I18nMap,
    http::bookables::{
        asset_types::{BookableTypePermRow, PostBookableTypeSchema, insert_asset_type},
        handlers::{PostBookableSchema, insert_bookable},
        schemas::{NewSchemaInline, insert_schema},
    },
};

pub struct AppointmentImport {
    pub asset_id: i32,
    /// None for maintenance
    pub account_id: Option<Uuid>,
    pub begins: DateTime<Utc>,
    /// None = open-ended
    pub ends: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub activated: Option<DateTime<Utc>>,
    pub cancelled: Option<(DateTime<Utc>, CancellationReason)>,
    pub maintenance: Option<MaintenanceRefund>,
    pub transaction_id: Option<Uuid>,
}

impl Importer {
    pub async fn asset_type(
        &mut self,
        r: &Ref,
        name: I18nMap,
        permissions: Vec<BookableTypePermRow>,
    ) -> Result<i32> {
        if let Some(id) = self.existing(r, "asset_type").await? {
            return Ok(id);
        }

        let body = PostBookableTypeSchema {
            name,
            permissions: Some(permissions),
        };
        let id = insert_asset_type(self.conn(), body)
            .await
            .map_err(api_error)?;

        self.remember(Some(r), "asset_type", id).await?;
        Ok(id)
    }

    /// Imports an asset through the API's insert. Schemas are assigned separately.
    pub async fn asset(&mut self, r: &Ref, a: PostBookableSchema) -> Result<i32> {
        if let Some(id) = self.existing(r, "asset").await? {
            return Ok(id);
        }

        let id = insert_bookable(self.conn(), a).await.map_err(api_error)?.id;

        self.remember(Some(r), "asset", id).await?;
        Ok(id)
    }

    pub async fn schema(&mut self, r: &Ref, asset_type_id: i32, s: NewSchemaInline) -> Result<i32> {
        if let Some(id) = self.existing(r, "schema").await? {
            return Ok(id);
        }

        let id = insert_schema(self.conn(), s, asset_type_id)
            .await
            .map_err(api_error)?;

        self.remember(Some(r), "schema", id).await?;
        Ok(id)
    }

    pub async fn schema_assignment(&mut self, asset_id: i32, schema_id: i32, begins: NaiveDate) -> Result<()> {
        sqlx::query!(
            "INSERT INTO bookable_schema_assignments (begins, schema_id, asset_id) VALUES ($1, $2, $3)
             ON CONFLICT (begins, asset_id) DO UPDATE SET schema_id = $2",
            begins,
            schema_id,
            asset_id
        )
        .execute(self.conn())
        .await?;
        Ok(())
    }

    /// Imports an appointment. Overlapping active ones are skipped, misaligned slots only warned about.
    pub async fn appointment(&mut self, r: &Ref, a: AppointmentImport) -> Result<Option<Uuid>> {
        if let Some(id) = self.existing(r, "appointment").await? {
            return Ok(Some(id));
        }

        if a.cancelled.is_none()
            && let Some(other) = sqlx::query_scalar!(
                "SELECT id FROM bookable_appointments
                 WHERE asset_id = $1 AND cancelled_at IS NULL AND during && tstzrange($2, $3, '[)')",
                a.asset_id,
                a.begins,
                a.ends
            )
            .fetch_optional(self.conn())
            .await?
        {
            self.warn(Some(r), format!("overlaps {other}, skipped"));
            return Ok(None);
        }

        // The slot has to exist in the schema assigned at that point
        if a.maintenance.is_none() {
            let aligned = sqlx::query_scalar!(
                r#"SELECT EXISTS (
                    SELECT 1 FROM (
                        SELECT bs.schedule FROM bookable_schema_assignments bsa
                        JOIN bookable_schemas bs ON bs.id = bsa.schema_id
                        WHERE bsa.asset_id = $1 AND bsa.begins <= $2::timestamptz::date
                        ORDER BY bsa.begins DESC LIMIT 1
                    ) s, slots_from_schedule(s.schedule, $2::timestamptz::date) sl
                    WHERE sl.range = tstzrange($2, $3, '[)')
                ) AS "aligned!""#,
                a.asset_id,
                a.begins,
                a.ends
            )
            .fetch_one(self.conn())
            .await?;

            if !aligned {
                self.warn(Some(r), format!("{} doesn't match a schema slot", a.begins));
            }
        }

        let (cancelled_at, cancellation_reason) = a.cancelled.unzip();
        let id = sqlx::query_scalar!(
            "INSERT INTO bookable_appointments (asset_id, account_id, during, created_at, activated, cancelled_at, cancellation_reason, maintenance, transaction_id)
             VALUES ($1, $2, tstzrange($3, $4, '[)'), $5, $6, $7, $8, $9, $10) RETURNING id",
            a.asset_id,
            a.account_id,
            a.begins,
            a.ends,
            a.created_at,
            a.activated,
            cancelled_at,
            cancellation_reason as Option<CancellationReason>,
            a.maintenance as Option<MaintenanceRefund>,
            a.transaction_id
        )
        .fetch_one(self.conn())
        .await?;

        self.remember(Some(r), "appointment", id).await?;
        Ok(Some(id))
    }
}
