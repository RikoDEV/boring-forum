//! The forum as an OAuth 2 provider: clients, codes and access tokens.

use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct OAuthClient {
    pub client_id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
}

pub async fn create_client(
    pool: &PgPool,
    client_id: &str,
    secret_digest: &str,
    name: &str,
    uris: &[String],
) -> Result<(), Error> {
    sqlx::query("INSERT INTO oauth_clients (client_id, secret_digest, name, redirect_uris) VALUES ($1, $2, $3, $4)")
        .bind(client_id)
        .bind(secret_digest)
        .bind(name)
        .bind(uris)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn clients(pool: &PgPool) -> Result<Vec<OAuthClient>, Error> {
    sqlx::query_as("SELECT client_id, name, redirect_uris FROM oauth_clients ORDER BY id").fetch_all(pool).await
}

pub async fn client(pool: &PgPool, client_id: &str) -> Result<Option<OAuthClient>, Error> {
    sqlx::query_as("SELECT client_id, name, redirect_uris FROM oauth_clients WHERE client_id = $1")
        .bind(client_id)
        .fetch_optional(pool)
        .await
}

pub async fn client_secret_digest(pool: &PgPool, client_id: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT secret_digest FROM oauth_clients WHERE client_id = $1")
        .bind(client_id)
        .fetch_optional(pool)
        .await
}

pub async fn delete_client(pool: &PgPool, client_id: &str) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM api_keys WHERE oauth_client = $1").bind(client_id).execute(&mut *tx).await?;
    let n = sqlx::query("DELETE FROM oauth_clients WHERE client_id = $1")
        .bind(client_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(n > 0)
}

pub struct NewCode<'a> {
    pub digest: &'a str,
    pub client_id: &'a str,
    pub user_id: i64,
    pub scope: &'a str,
    pub redirect_uri: &'a str,
    pub challenge: &'a str,
}

pub async fn create_code(pool: &PgPool, c: &NewCode<'_>) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO oauth_codes (code_digest, client_id, user_id, scope, redirect_uri, challenge, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, now() + interval '2 minutes')",
    )
    .bind(c.digest)
    .bind(c.client_id)
    .bind(c.user_id)
    .bind(c.scope)
    .bind(c.redirect_uri)
    .bind(c.challenge)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Takes a code; deleting it here makes each one single-use even if the checks then fail.
pub async fn take_code(pool: &PgPool, digest: &str) -> Result<Option<(String, i64, String, String, String)>, Error> {
    sqlx::query_as(
        "DELETE FROM oauth_codes WHERE code_digest = $1 AND expires_at > now()
         RETURNING client_id, user_id, scope, redirect_uri, challenge",
    )
    .bind(digest)
    .fetch_optional(pool)
    .await
}

pub async fn create_access_token(
    pool: &PgPool,
    user: i64,
    client: &str,
    digest: &str,
    scope: &str,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO api_keys (user_id, name, key_digest, scope, expires_at, oauth_client)
         VALUES ($1, $2, $3, $4, now() + interval '1 hour', $2)",
    )
    .bind(user)
    .bind(client)
    .bind(digest)
    .bind(scope)
    .execute(pool)
    .await
    .map(|_| ())
}
