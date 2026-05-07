use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    pub usage: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

pub async fn list_by_conversation(pool: &PgPool, conversation_id: Uuid) -> Result<Vec<Message>, sqlx::Error> {
    sqlx::query_as::<_, Message>(
        "SELECT * FROM messages WHERE conversation_id = $1 ORDER BY created_at ASC",
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await
}

pub async fn insert(pool: &PgPool, conversation_id: Uuid, role: &str, content: &str) -> Result<Message, sqlx::Error> {
    sqlx::query_as::<_, Message>(
        "INSERT INTO messages (conversation_id, role, content)
         VALUES ($1, $2, $3)
         RETURNING *",
    )
    .bind(conversation_id)
    .bind(role)
    .bind(content)
    .fetch_one(pool)
    .await
}

pub async fn insert_with_usage(
    pool: &PgPool,
    conversation_id: Uuid,
    role: &str,
    content: &str,
    usage: serde_json::Value,
) -> Result<Message, sqlx::Error> {
    sqlx::query_as::<_, Message>(
        "INSERT INTO messages (conversation_id, role, content, usage)
         VALUES ($1, $2, $3, $4)
         RETURNING *",
    )
    .bind(conversation_id)
    .bind(role)
    .bind(content)
    .bind(usage)
    .fetch_one(pool)
    .await
}
