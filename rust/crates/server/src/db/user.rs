use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub clerk_id: String,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub plan: String,
    pub use_case: Option<String>,
    pub occupation: Option<String>,
    pub goals: Option<String>,
    pub onboarded_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn get_by_clerk_id(pool: &PgPool, clerk_id: &str) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE clerk_id = $1")
        .bind(clerk_id)
        .fetch_optional(pool)
        .await
}

pub async fn create(
    pool: &PgPool,
    clerk_id: &str,
    email: &str,
    name: Option<&str>,
    avatar_url: Option<&str>,
) -> Result<User, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "INSERT INTO users (clerk_id, email, name, avatar_url)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (clerk_id) DO UPDATE
           SET email = EXCLUDED.email,
               name = COALESCE(EXCLUDED.name, users.name),
               avatar_url = COALESCE(EXCLUDED.avatar_url, users.avatar_url)
         RETURNING *",
    )
    .bind(clerk_id)
    .bind(email)
    .bind(name)
    .bind(avatar_url)
    .fetch_one(pool)
    .await
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfile {
    pub name: Option<String>,
    pub use_case: Option<String>,
    pub occupation: Option<String>,
    pub goals: Option<String>,
    pub mark_onboarded: Option<bool>,
}

pub async fn update_profile(pool: &PgPool, user_id: Uuid, patch: UpdateProfile) -> Result<User, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "UPDATE users SET
           name        = COALESCE($2, name),
           use_case    = COALESCE($3, use_case),
           occupation  = COALESCE($4, occupation),
           goals       = COALESCE($5, goals),
           onboarded_at = CASE WHEN $6 THEN now() ELSE onboarded_at END
         WHERE id = $1
         RETURNING *",
    )
    .bind(user_id)
    .bind(patch.name)
    .bind(patch.use_case)
    .bind(patch.occupation)
    .bind(patch.goals)
    .bind(patch.mark_onboarded.unwrap_or(false))
    .fetch_one(pool)
    .await
}

pub async fn delete_by_clerk_id(pool: &PgPool, clerk_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM users WHERE clerk_id = $1")
        .bind(clerk_id)
        .execute(pool)
        .await?;
    Ok(())
}
