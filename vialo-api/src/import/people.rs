use anyhow::Result;
use chrono::NaiveDate;
use uuid::Uuid;

use super::{Importer, Ref, api_error};
use crate::{
    http::people::{
        groups::{CreateGroupSchema, insert_group, replace_app_roles},
        handlers::insert_person,
        schemas::{CreateRoomSchema, CreateUserSchema},
    },
    permissions::GroupRole,
};

/// What the API can't set on creation
pub struct PersonImport {
    pub person: CreateUserSchema,
    pub membership_start: Option<NaiveDate>,
    pub manually_suspended: bool,
}

impl Importer {
    /// Imports a person through the API's insert. A taken email is dropped with a warning.
    pub async fn person(&mut self, r: &Ref, mut p: PersonImport) -> Result<Uuid> {
        if let Some(id) = self.existing(r, "person").await? {
            return Ok(id);
        }

        // A failed insert would abort the whole import
        if let Some(email) = &p.person.email
            && let Some(holder) = sqlx::query_scalar!(
                "SELECT id FROM accounts_people WHERE email = $1",
                email
            )
            .fetch_optional(self.conn())
            .await?
        {
            self.warn(Some(r), format!("email {email} already taken by {holder}, imported without"));
            p.person.email = None;
        }

        let id = insert_person(self.trans(), p.person).await.map_err(api_error)?.id;

        sqlx::query!(
            "UPDATE accounts_people SET (membership_start, manually_suspended) = ($2, $3) WHERE id = $1",
            id,
            p.membership_start,
            p.manually_suspended
        )
        .execute(self.conn())
        .await?;

        self.remember(Some(r), "person", id).await?;
        Ok(id)
    }

    /// Creates or updates a room by label.
    pub async fn room(&mut self, room: CreateRoomSchema) -> Result<Uuid> {
        let id = sqlx::query_scalar!(
            "INSERT INTO res_rooms (label, floor, capacity) VALUES ($1, $2, $3)
             ON CONFLICT (label) DO UPDATE SET (floor, capacity) = (EXCLUDED.floor, EXCLUDED.capacity)
             RETURNING id",
            room.label,
            room.floor,
            room.capacity
        )
        .fetch_one(self.conn())
        .await?;
        Ok(id)
    }

    /// Imports a group, unlike the API's POST including its app roles.
    pub async fn group(&mut self, r: &Ref, mut g: CreateGroupSchema) -> Result<Uuid> {
        if let Some(id) = self.existing(r, "group").await? {
            return Ok(id);
        }

        let app_roles = g.app_roles.take();
        let id = insert_group(self.conn(), g).await.map_err(api_error)?.id;
        if let Some(app_roles) = app_roles {
            replace_app_roles(self.conn(), id, app_roles).await.map_err(api_error)?;
        }

        self.remember(Some(r), "group", id).await?;
        Ok(id)
    }

    pub async fn membership(&mut self, group: Uuid, account: Uuid, role: GroupRole) -> Result<()> {
        sqlx::query!(
            "INSERT INTO account_group_memberships (group_id, account_id, role) VALUES ($1, $2, $3)
             ON CONFLICT (account_id, group_id) DO NOTHING",
            group,
            account,
            role as GroupRole
        )
        .execute(self.conn())
        .await?;
        Ok(())
    }
}
