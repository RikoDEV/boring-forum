//! Site settings storage: overrides on top of the schema in `bf_core::settings`, with a
//! LISTEN/NOTIFY signal so other nodes reload.

use bf_core::settings::Settings;
use serde_json::Value;
use sqlx::PgPool;

type Error = sqlx::Error;

/// Settings that hold a key or secret: sealed with `BF_SECRET_KEY` in the database (and so in
/// every backup), shown to the panel only as set or not set.
pub fn is_secret(key: &str) -> bool {
    matches!(
        key,
        "ai_api_key"
            | "ai_embedding_api_key"
            | "akismet_key"
            | "gif_api_key"
            | "mailchimp_webhook_secret"
            | "imgproxy_key"
            | "imgproxy_salt"
            | "chat_webhook_url"
    )
}

/// Loads stored overrides on top of the schema defaults, opening the sealed secrets. One that
/// cannot be opened (its key is not configured) is left out rather than used as a credential.
pub async fn load(pool: &PgPool) -> Result<Settings, Error> {
    let rows: Vec<(String, Value)> = sqlx::query_as("SELECT key, value FROM site_settings").fetch_all(pool).await?;
    let rows = rows
        .into_iter()
        .filter_map(|(key, value)| match (is_secret(&key), value.as_str()) {
            (true, Some(stored)) => match bf_core::secretbox::open(stored) {
                Some(plain) => Some((key, Value::String(plain))),
                None => {
                    tracing::error!(%key, "setting is sealed with a key that is not configured");
                    None
                }
            },
            _ => Some((key, value)),
        })
        .collect::<Vec<_>>();
    Ok(Settings::from_overrides(rows))
}

/// Stores one already-validated value and notifies other nodes.
pub async fn save(pool: &PgPool, key: &str, value: &Value) -> Result<(), Error> {
    let sealed;
    let value = match value.as_str() {
        Some(plain) if is_secret(key) && !plain.is_empty() => {
            sealed = Value::String(bf_core::secretbox::seal(plain));
            &sealed
        }
        _ => value,
    };
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO site_settings (key, value, updated_at) VALUES ($1, $2, now())
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(key)
    .bind(value)
    .execute(&mut *tx)
    .await?;
    sqlx::query("SELECT pg_notify('bf_settings', $1)").bind(key).execute(&mut *tx).await?;
    tx.commit().await
}
