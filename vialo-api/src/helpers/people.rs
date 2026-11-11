use crate::{helpers::encryption, helpers::random};
use anyhow::Result;
use ory_kratos_client::{
    apis::{configuration::Configuration, frontend_api, identity_api},
    models::{
        UpdateRecoveryFlowBody, UpdateRecoveryFlowWithLinkMethod,
        update_recovery_flow_with_link_method::MethodEnum,
    },
};
use rand::{RngExt, rng};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use utoipa::ToSchema;
use uuid::Uuid;

/// A person's amenities login (printer, kiosk).
pub struct AmenitiesLogin {
    pub username: String,
    pub pin: String,
}

/// Generate and store a person's amenities login, replacing any existing pair.
pub async fn generate_amenities_login(
    id: Uuid,
    trans: &mut Transaction<'_, Postgres>,
) -> Result<AmenitiesLogin> {
    let (username, pin) = {
        let mut rng = rng();
        let username = random::unambiguous_string(8);
        let pin: String = (0..6)
            .map(|_| char::from(b'0' + rng.random_range(0..10u8)))
            .collect();
        (username, pin)
    };

    sqlx::query!(
        "UPDATE accounts_people SET (amenities_username, amenities_pin) = ($1, $2) WHERE id = $3",
        username,
        encryption::encrypt(&pin)?,
        id
    )
    .execute(&mut **trans)
    .await?;

    Ok(AmenitiesLogin { username, pin })
}

/// Builds a Kratos client config with `unix://` URLs going over a Unix socket
pub fn kratos_configuration(url: &str) -> Configuration {
    let url = url.trim_end_matches('/');
    let Some(socket_path) = url.strip_prefix("unix://") else {
        return Configuration {
            base_path: url.to_string(),
            ..Default::default()
        };
    };
    #[cfg(unix)]
    {
        Configuration {
            base_path: "http://localhost".to_string(), // dud
            client: reqwest_kratos::Client::builder()
                .unix_socket(socket_path)
                .build()
                .expect("Failed to build Unix socket HTTP client for Kratos"),
            ..Default::default()
        }
    }
    #[cfg(not(unix))]
    panic!("Kratos URL {socket_path} is a Unix socket but this platform does not support them")
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct IdentityModel {
    pub id: Uuid,
    pub account_id: Option<Uuid>,
    pub full_name: Option<String>,
    pub display_name: Option<String>,
    #[schema(format = Email)]
    pub email: Option<String>,
    pub phone: Option<String>,
    pub room: Option<String>,
}

pub async fn delete_identity(
    id: Uuid,
    kratos_admin: Option<&Configuration>,
    trans: &mut Transaction<'_, Postgres>,
) -> Result<()> {
    sqlx::query!(r#"DELETE FROM identities i WHERE i.id = $1"#, id)
        .execute(&mut **trans)
        .await?;

    if let Some(kratos_admin) = kratos_admin {
        identity_api::delete_identity(kratos_admin, &id.to_string()).await?;
    }

    Ok(())
}

/// Email a recovery link
pub async fn reset_identity_password(
    id: Uuid,
    kratos_frontend: &Configuration,
    db: impl sqlx::PgExecutor<'_>,
) -> Result<()> {
    let email = sqlx::query_scalar!(r#"SELECT i.email FROM identities i WHERE i.id = $1"#, id)
        .fetch_one(db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Identity {id} has no email"))?;

    let flow = frontend_api::create_native_recovery_flow(kratos_frontend).await?;
    let body = UpdateRecoveryFlowBody::Link(Box::new(UpdateRecoveryFlowWithLinkMethod::new(
        email,
        MethodEnum::Link,
    )));
    frontend_api::update_recovery_flow(kratos_frontend, &flow.id, body, None, None).await?;

    Ok(())
}

pub async fn update_identity(
    body: IdentityModel,
    trans: &mut Transaction<'_, Postgres>,
) -> Result<()> {
    // Insert/Update the identity
    sqlx::query!(
        "INSERT INTO identities (id, email, full_name, display_name, phone, room) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (id) DO UPDATE SET (email, full_name, display_name, phone) = ($2, $3, $4, $5)",
        body.id, body.email, body.full_name, body.display_name, body.phone, body.room
    ).execute(&mut **trans).await?;

    // Email and display_name can be changed by the user
    sqlx::query!(
        "UPDATE accounts_people SET email = $1, display_name = $2 WHERE auth_id = $3",
        body.email,
        body.display_name,
        body.id
    )
    .execute(&mut **trans)
    .await?;

    // Create initial admin account if needed
    if let (Some(email), Ok(initial_admin_email)) =
        (&body.email, std::env::var("INITIAL_ADMIN_EMAIL"))
        && email.eq_ignore_ascii_case(&initial_admin_email)
    {
        // Check if there are any users bound to identities yet
        let has_users = sqlx::query_scalar!(
                r#"SELECT EXISTS (SELECT 1 FROM accounts_people WHERE auth_id IS NOT NULL) AS "exists!""#
            )
            .fetch_optional(&mut **trans)
            .await?
            .unwrap_or(false);

        if !has_users {
            tracing::info!("Bootstrapping initial admin account for {}", email);

            // Create the account and bind the auth_id
            let account_id = sqlx::query_scalar!(
                    "INSERT INTO accounts_people (full_name, email, label, membership_end, auth_id) VALUES ($1, $2, 'Created by Vialo during setup', 'infinity', $3) RETURNING id",
                    body.full_name.as_deref().unwrap_or("Admin"),
                    email,
                    body.id
                ).fetch_one(&mut **trans).await?;

            // Create the group or reuse it if a previous bootstrap left one behind
            let group_id = match sqlx::query_scalar!(
                "SELECT id FROM account_groups WHERE label = 'AdminAG'"
            )
            .fetch_optional(&mut **trans)
            .await?
            {
                Some(id) => id,
                None => sqlx::query_scalar!(
                    "INSERT INTO account_groups (label, public) VALUES ('AdminAG', true) RETURNING id"
                )
                .fetch_one(&mut **trans)
                .await?,
            };

            // Add the account to the group
            sqlx::query!(
                    "insert into account_group_memberships_manual (group_id, account_id, role) VALUES ($1, $2, 'manager');",
                    group_id, account_id
                ).execute(&mut **trans).await?;

            // Give the group every app role
            sqlx::query!(
                    "INSERT INTO account_group_app_roles (group_id, role) (select $1 as group_id, unnest(enum_range(null, null::app_role)) as role)",
                    group_id
                ).execute(&mut **trans).await?;
        }
    }

    Ok(())
}
