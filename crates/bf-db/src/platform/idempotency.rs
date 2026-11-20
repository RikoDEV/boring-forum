//! `Idempotency-Key` storage for API writes.

use sqlx::PgPool;

type Error = sqlx::Error;

pub enum Claim {
    /// First use of the key: run the request.
    New,
    /// The same request is still running.
    InFlight,
    /// The key was used for a different request.
    Mismatch,
    /// The stored response: (status, content type, body).
    Done(i16, Option<String>, Vec<u8>),
}

pub async fn claim(pool: &PgPool, key: &str, fingerprint: &str) -> Result<Claim, Error> {
    let inserted = sqlx::query!(
        "INSERT INTO api_idempotency (key_digest, fingerprint) VALUES ($1, $2) ON CONFLICT (key_digest) DO NOTHING",
        key,
        fingerprint
    )
    .execute(pool)
    .await?
    .rows_affected();
    if inserted == 1 {
        return Ok(Claim::New);
    }
    let row = sqlx::query!(
        "SELECT fingerprint, completed, status, content_type, body FROM api_idempotency WHERE key_digest = $1",
        key
    )
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some(r) if r.fingerprint != fingerprint => Claim::Mismatch,
        // Not completed yet, or expired between the insert and the read (treated as new on the
        // next attempt).
        None => Claim::InFlight,
        Some(r) if !r.completed => Claim::InFlight,
        Some(r) => Claim::Done(r.status.unwrap_or(200), r.content_type, r.body.unwrap_or_default()),
    })
}

pub async fn complete(pool: &PgPool, key: &str, status: i16, ctype: Option<&str>, body: &[u8]) -> Result<(), Error> {
    sqlx::query!(
        "UPDATE api_idempotency SET completed = true, status = $2, content_type = $3, body = $4 WHERE key_digest = $1",
        key,
        status,
        ctype,
        body
    )
    .execute(pool)
    .await
    .map(|_| ())
}

/// A failed attempt (server error) frees the key, so the client may retry it.
pub async fn release(pool: &PgPool, key: &str) -> Result<(), Error> {
    sqlx::query!("DELETE FROM api_idempotency WHERE key_digest = $1", key).execute(pool).await.map(|_| ())
}
