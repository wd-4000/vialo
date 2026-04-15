use anyhow::Result;
use reqwest::Url;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::hooks::IdentityModel;

pub async fn delete_identity(id: Uuid, trans: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query!(r#"DELETE FROM identities i WHERE i.id = $1"#, id)
        .execute(&mut **trans)
        .await?;

    if let Ok(kratos_url) = std::env::var("ORY_KRATOS_ADMIN_URL") {
        let delete_url = Url::parse(&kratos_url)?
            .join("admin/identities/")?
            .join(&id.to_string())?;

        let kratos_response = reqwest::Client::new().delete(delete_url).send().await?;

        kratos_response.error_for_status()?;
    }

    Ok(())
}

pub async fn update_identity(
    body: IdentityModel,
    trans: &mut Transaction<'_, Postgres>,
) -> Result<()> {
    // Insert/Update the identity
    sqlx::query!(
        "INSERT INTO identities (id, email, full_name) VALUES ($1, $2, $3) ON CONFLICT (id) DO UPDATE SET (email, full_name) = ($2, $3)",
        body.id, body.email, body.full_name
    ).execute(&mut **trans).await?;

    // Create initial admin account if needed
    if let (Some(email), Ok(initial_admin_email)) =
        (&body.email, std::env::var("INITIAL_ADMIN_EMAIL"))
    {
        if email.eq_ignore_ascii_case(&initial_admin_email) {
            // Check if there are any users bound to identities yet
            let has_users = sqlx::query_scalar!(
                r#"SELECT EXISTS (SELECT 1 FROM accounts_people WHERE auth_id IS NOT NULL) AS "exists!""#
            )
            .fetch_one(&mut **trans)
            .await
            .unwrap_or(true);

            if !has_users {
                tracing::info!("Bootstrapping initial admin account for {}", email);

                // Create the account and bind the auth_id
                let account_id = sqlx::query_scalar!(
                    "INSERT INTO accounts_people (full_name, email, label, membership_end, auth_id) VALUES ($1, $2, 'Created by Vialo during setup', 'infinity', $3) RETURNING id",
                    body.full_name.as_deref().unwrap_or("Admin"),
                    email,
                    body.id
                ).fetch_one(&mut **trans).await?;

                // Create the group
                let group_id = sqlx::query_scalar!(
                    "insert into account_groups (label,public) values ('AdminAG', true) RETURNING id",
                ).fetch_one(&mut **trans).await?;

                // Add the account to the group
                sqlx::query!(
                    "insert into account_group_memberships (group_id, account_id, role) VALUES ($1, $2, 'manager');",
                    group_id, account_id
                ).execute(&mut **trans).await?;

                // Give the group every app role
                sqlx::query!(
                    "INSERT INTO account_group_app_roles (group_id, role) (select $1 as group_id, unnest(enum_range(null, null::app_role)) as role)",
                    group_id
                ).execute(&mut **trans).await?;
            }
        }
    }

    Ok(())
}
