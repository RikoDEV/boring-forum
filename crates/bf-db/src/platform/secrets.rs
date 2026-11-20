//! Key rotation for the columns encrypted at rest.
//!
//! `boring-forum rotate-secrets` rewrites every sensitive column with the current
//! `BF_SECRET_KEY`. Run it after a key change, with the retired key still listed in
//! `BF_SECRET_KEY_OLD` so the old values can be read; once it reports no rows left on an old
//! key, drop `BF_SECRET_KEY_OLD`. Rows already on the current key are left alone, so the command
//! is safe to re-run and cheap when there is nothing to do.

use bf_core::secretbox;
use crate::admin::audit::{Entry, record};
use sqlx::Error;
use sqlx::postgres::PgPool;

/// What a rotation did, per column.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rotated {
    pub totp_secrets: u64,
    pub webhook_secrets: u64,
    /// Private keys of the federation actors (`ap_keys.private_pem`).
    pub actor_keys: u64,
    /// Integration keys in `site_settings`.
    pub settings: u64,
    /// Values no key in the ring can open. They are left untouched: the key that sealed them is
    /// gone, and overwriting would destroy the only copy.
    pub unreadable: u64,
}

impl std::fmt::Display for Rotated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "re-encrypted {} TOTP secret(s), {} webhook secret(s), {} federation actor key(s) and {} integration setting(s) with the current key",
            self.totp_secrets, self.webhook_secrets, self.actor_keys, self.settings
        )?;
        if self.unreadable > 0 {
            write!(f, "; {} value(s) could not be opened and were left as they are", self.unreadable)?;
        }
        Ok(())
    }
}

/// Re-seals every stale value. Each row is read, opened and written back in one transaction, so
/// a crash halfway leaves a mix of keys rather than a lost secret -- which the next run fixes.
pub async fn rotate(pool: &PgPool) -> Result<Rotated, Error> {
    let Some(ring) = secretbox::keyring() else {
        return Err(Error::Configuration("BF_SECRET_KEY is not set, so there is no key to rotate to".into()));
    };
    let mut out = Rotated::default();

    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, totp_secret FROM users WHERE totp_secret IS NOT NULL").fetch_all(pool).await?;
    for (id, stored) in rows {
        if !ring.stale(&stored) {
            continue;
        }
        if let Some(plain) = ring.open(&stored) {
            sqlx::query("UPDATE users SET totp_secret = $2 WHERE id = $1 AND totp_secret = $3")
                .bind(id)
                .bind(ring.seal(&plain))
                .bind(&stored)
                .execute(pool)
                .await?;
            out.totp_secrets += 1;
        } else {
            tracing::error!(user = id, "TOTP secret is sealed with a key that is not configured");
            out.unreadable += 1;
        }
    }

    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, secret FROM webhooks").fetch_all(pool).await?;
    for (id, stored) in rows {
        if !ring.stale(&stored) {
            continue;
        }
        if let Some(plain) = ring.open(&stored) {
            sqlx::query("UPDATE webhooks SET secret = $2 WHERE id = $1 AND secret = $3")
                .bind(id)
                .bind(ring.seal(&plain))
                .bind(&stored)
                .execute(pool)
                .await?;
            out.webhook_secrets += 1;
        } else {
            tracing::error!(webhook = id, "webhook secret is sealed with a key that is not configured");
            out.unreadable += 1;
        }
    }

    let rows: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, url, token FROM chat_integration_rules").fetch_all(pool).await?;
    for (id, url, token) in rows {
        let mut seal_again = |stored: &str| -> Option<String> {
            if stored.is_empty() || !ring.stale(stored) {
                return Some(stored.to_owned());
            }
            let opened = ring.open(stored);
            if opened.is_none() {
                tracing::error!(rule = id, "chat integration secret is sealed with a key that is not configured");
                out.unreadable += 1;
            }
            opened.map(|plain| ring.seal(&plain))
        };
        let (Some(new_url), Some(new_token)) = (seal_again(&url), seal_again(&token)) else { continue };
        if new_url != url || new_token != token {
            sqlx::query("UPDATE chat_integration_rules SET url = $2, token = $3 WHERE id = $1")
                .bind(id)
                .bind(new_url)
                .bind(new_token)
                .execute(pool)
                .await?;
        }
    }

    // The private keys federation signs with (sealed by `federation::signatures::key_for`).
    let rows: Vec<(String, i64, String)> =
        sqlx::query_as("SELECT owner_type, owner_id, private_pem FROM ap_keys").fetch_all(pool).await?;
    for (owner_type, owner_id, stored) in rows {
        if !ring.stale(&stored) {
            continue;
        }
        if let Some(plain) = ring.open(&stored) {
            sqlx::query("UPDATE ap_keys SET private_pem = $3 WHERE owner_type = $1 AND owner_id = $2 AND private_pem = $4")
                .bind(&owner_type)
                .bind(owner_id)
                .bind(ring.seal(&plain))
                .bind(&stored)
                .execute(pool)
                .await?;
            out.actor_keys += 1;
        } else {
            tracing::error!(owner_type = %owner_type, owner_id, "federation actor key is sealed with a key that is not configured");
            out.unreadable += 1;
        }
    }

    // The integration keys of the settings page (AI, Akismet, imgproxy, ...).
    let rows: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT key, value FROM site_settings WHERE jsonb_typeof(value) = 'string'").fetch_all(pool).await?;
    for (key, value) in rows {
        let Some(stored) = value.as_str().filter(|s| !s.is_empty() && crate::settings::is_secret(&key)) else { continue };
        if !ring.stale(stored) {
            continue;
        }
        if let Some(plain) = ring.open(stored) {
            sqlx::query("UPDATE site_settings SET value = $2 WHERE key = $1 AND value = $3")
                .bind(&key)
                .bind(serde_json::Value::String(ring.seal(&plain)))
                .bind(&value)
                .execute(pool)
                .await?;
            out.settings += 1;
        } else {
            tracing::error!(%key, "setting is sealed with a key that is not configured");
            out.unreadable += 1;
        }
    }

    record(
        pool,
        Entry::new("security.rotate_secrets").details(serde_json::json!({
        "totp_secrets": out.totp_secrets,
        "webhook_secrets": out.webhook_secrets,
        "actor_keys": out.actor_keys,
        "settings": out.settings,
        "unreadable": out.unreadable,
        "key_id": ring.current_id(),
        })),
    ).await?;
    Ok(out)
}
