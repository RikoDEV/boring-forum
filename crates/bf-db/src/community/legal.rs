//! Legal documents and acceptances, likes and "top" lists.

use chrono::{DateTime, Utc};
use serde::Serialize;
pub use sqlx::Error;
use crate::admin::audit::{Entry, record};
use sqlx::{FromRow, PgPool};

/// Documents a member must accept (guidelines are informational).
pub const REQUIRED: [&str; 2] = ["terms", "privacy"];

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct LegalDoc {
    pub id: i64,
    pub kind: String,
    pub version: i32,
    pub body_md: String,
    pub body_html: String,
    pub created_at: DateTime<Utc>,
}

pub async fn latest(pool: &PgPool, kind: &str) -> Result<Option<LegalDoc>, Error> {
    sqlx::query_as(
        "SELECT id, kind, version, body_md, body_html, created_at FROM legal_documents
         WHERE kind = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(kind)
    .fetch_optional(pool)
    .await
}

/// The kinds that have at least one published version.
pub async fn published_kinds(pool: &PgPool) -> Result<Vec<String>, Error> {
    sqlx::query_scalar("SELECT DISTINCT kind FROM legal_documents").fetch_all(pool).await
}

/// Publishes a new version; members are asked to accept it again.
pub async fn publish(pool: &PgPool, kind: &str, md: &str, html: &str, actor: i64) -> Result<i32, Error> {
    let mut tx = pool.begin().await?;
    let version: i32 = sqlx::query_scalar(
        "INSERT INTO legal_documents (kind, version, body_md, body_html)
         SELECT $1, coalesce(max(version), 0) + 1, $2, $3 FROM legal_documents WHERE kind = $1
         RETURNING version",
    )
    .bind(kind)
    .bind(md)
    .bind(html)
    .fetch_one(&mut *tx)
    .await?;
    record(&mut *tx, Entry::new("legal.publish").by(actor).on(format!("legal:{kind}")).details(serde_json::json!({ "version": version }))).await?;
    tx.commit().await?;
    Ok(version)
}

/// The latest version of each required document, i.e. what a new member accepts.
pub async fn current_required(pool: &PgPool) -> Result<Vec<LegalDoc>, Error> {
    sqlx::query_as(
        "SELECT DISTINCT ON (kind) id, kind, version, body_md, body_html, created_at
         FROM legal_documents WHERE kind = ANY($1) ORDER BY kind, version DESC",
    )
    .bind(&REQUIRED[..])
    .fetch_all(pool)
    .await
}

/// Latest required documents the user has not accepted yet.
pub async fn pending(pool: &PgPool, user_id: i64) -> Result<Vec<LegalDoc>, Error> {
    sqlx::query_as(
        "SELECT * FROM (SELECT DISTINCT ON (kind) id, kind, version, body_md, body_html, created_at
                        FROM legal_documents WHERE kind = ANY($2) ORDER BY kind, version DESC) d
         WHERE NOT EXISTS (SELECT 1 FROM user_legal_acceptances a WHERE a.user_id = $1 AND a.document_id = d.id)",
    )
    .bind(user_id)
    .bind(&REQUIRED[..])
    .fetch_all(pool)
    .await
}

pub async fn accept(pool: &PgPool, user_id: i64, doc_ids: &[i64]) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO user_legal_acceptances (user_id, document_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(doc_ids)
    .execute(pool)
    .await
    .map(|_| ())
}
