//! Provisioning Kratos identities for imported people. Runs after the import committed,
//! one account at a time, since Kratos can't share the transaction.

use anyhow::{Result, bail};
use ory_kratos_client::{
    apis::{configuration::Configuration, identity_api},
    models::{
        CreateIdentityBody, IdentityWithCredentials, IdentityWithCredentialsPassword,
        IdentityWithCredentialsPasswordConfig, VerifiableIdentityAddress,
        verifiable_identity_address::ViaEnum,
    },
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

pub struct IdentityImport {
    pub account: Uuid,
    pub email: String,
    pub full_name: String,
    pub phone: Option<String>,
    pub room: Option<String>,
    /// Any hash format Kratos can import
    pub password_hash: Option<String>,
}

#[derive(Debug)]
pub enum Provisioned {
    AlreadyLinked(Uuid),
    /// An identity with that email already existed
    Linked(Uuid),
    Created(Uuid),
}

/// Creates (or finds by email) the identity and links it
pub async fn provision_identity(
    pool: &PgPool,
    kratos_admin: &Configuration,
    schema_id: &str,
    i: IdentityImport,
) -> Result<Provisioned> {
    if let Some(auth_id) = sqlx::query_scalar!(
        "SELECT auth_id FROM accounts_people WHERE id = $1",
        i.account
    )
    .fetch_one(pool)
    .await?
    {
        return Ok(Provisioned::AlreadyLinked(auth_id));
    }

    let existing = identity_api::list_identities(
        kratos_admin,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&i.email),
        None,
        None,
        None,
    )
    .await?
    .into_iter()
    .next();

    let (identity_id, provisioned): (Uuid, fn(Uuid) -> Provisioned) = match existing {
        Some(identity) => (identity.id.parse()?, Provisioned::Linked),
        None => {
            let mut body = CreateIdentityBody::new(
                schema_id.to_string(),
                json!({
                    "email": i.email,
                    "full_name": i.full_name,
                    "phone": i.phone,
                    "room": i.room,
                }),
            );
            // The legacy system verified addresses already
            body.verifiable_addresses = Some(vec![VerifiableIdentityAddress::new(
                "completed".into(),
                i.email.clone(),
                true,
                ViaEnum::Email,
            )]);
            if let Some(hash) = i.password_hash {
                let mut config = IdentityWithCredentialsPasswordConfig::new();
                config.hashed_password = Some(hash);
                let mut password = IdentityWithCredentialsPassword::new();
                password.config = Some(Box::new(config));
                let mut credentials = IdentityWithCredentials::new();
                credentials.password = Some(Box::new(password));
                body.credentials = Some(Box::new(credentials));
            }

            let identity = identity_api::create_identity(kratos_admin, Some(body)).await?;
            (identity.id.parse()?, Provisioned::Created)
        }
    };

    let mut trans = pool.begin().await?;
    sqlx::query!("SELECT set_config('app.subsystem', 'import', true)")
        .fetch_one(&mut *trans)
        .await?;

    if let Some(other) = sqlx::query_scalar!(
        "SELECT id FROM accounts_people WHERE auth_id = $1",
        identity_id
    )
    .fetch_optional(&mut *trans)
    .await?
    {
        bail!(
            "identity {identity_id} for {} is already linked to {other}",
            i.email
        );
    }

    // Admin-created identities don't go through the update_identity hook
    sqlx::query!(
        "INSERT INTO identities (id, full_name, email, phone, room) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (id) DO NOTHING",
        identity_id,
        i.full_name,
        i.email,
        i.phone,
        i.room
    )
    .execute(&mut *trans)
    .await?;
    sqlx::query!(
        "UPDATE accounts_people SET auth_id = $1 WHERE id = $2 AND auth_id IS NULL",
        identity_id,
        i.account
    )
    .execute(&mut *trans)
    .await?;

    trans.commit().await?;
    Ok(provisioned(identity_id))
}
