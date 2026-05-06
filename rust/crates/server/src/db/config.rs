use sqlx::PgPool;

pub async fn get_active_model(pool: &PgPool) -> Result<String, sqlx::Error> {
    let row: (String,) = sqlx::query_as("SELECT value FROM config WHERE key = 'active_model'")
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

pub async fn set(pool: &PgPool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO config (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = $2")
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}
