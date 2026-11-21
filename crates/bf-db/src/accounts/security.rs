//! Account security queries: email tokens, the mail outbox, passwords, sessions and TOTP.

use crate::admin::audit::{Entry, record};
use bf_core::ids::UserId;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

/// Stores a one-time token digest for `purpose` ('verify', 'reset', 'email_change').
pub async fn create_email_token(
    pool: &PgPool,
    digest: &str,
    user_id: UserId,
    purpose: &str,
    payload: Option<&str>,
    ttl_minutes: i32,
) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO email_tokens (token_digest, user_id, purpose, payload, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5))",
    )
    .bind(digest)
    .bind(user_id)
    .bind(purpose)
    .bind(payload)
    .bind(ttl_minutes)
    .execute(&mut *tx)
    .await?;
    // At most five live links of a kind: a new one retires the oldest. Refusing the new one
    // instead would let anyone who knows an address use up its owner's links and so stop the
    // owner's own reset or sign-in mail from ever being sent.
    sqlx::query(
        "UPDATE email_tokens SET used_at = now()
         WHERE user_id = $1 AND purpose = $2 AND used_at IS NULL
           AND token_digest NOT IN (SELECT token_digest FROM email_tokens
                                    WHERE user_id = $1 AND purpose = $2 AND used_at IS NULL
                                    ORDER BY expires_at DESC LIMIT 5)",
    )
    .bind(user_id)
    .bind(purpose)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Uses a token once: returns its user and payload if it was valid, unused and unexpired.
pub async fn consume_email_token(
    pool: &PgPool,
    digest: &str,
    purpose: &str,
) -> Result<Option<(i64, Option<String>)>, Error> {
    sqlx::query_as(
        "UPDATE email_tokens SET used_at = now()
         WHERE token_digest = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > now()
         RETURNING user_id, payload",
    )
    .bind(digest)
    .bind(purpose)
    .fetch_optional(pool)
    .await
}

/// The account a valid, unused token belongs to, without using it.
pub async fn email_token_owner(pool: &PgPool, digest: &str, purpose: &str) -> Result<Option<UserId>, Error> {
    sqlx::query_scalar(
        "SELECT user_id FROM email_tokens
         WHERE token_digest = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(digest)
    .bind(purpose)
    .fetch_optional(pool)
    .await
}

/// Checks a token without using it (to show the reset form).
pub async fn email_token_valid(pool: &PgPool, digest: &str, purpose: &str) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM email_tokens
           WHERE token_digest = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > now())",
    )
    .bind(digest)
    .bind(purpose)
    .fetch_one(pool)
    .await
}

#[derive(Debug, Clone, FromRow)]
pub struct AccountInfo {
    pub id: UserId,
    pub username: String,
    pub email: String,
    pub email_verified: bool,
    pub password_hash: Option<String>,
    /// Held in the clear here and encrypted in the column. Both readers below open it,
    /// so a handler never sees the sealed form; `None` also covers a secret this process has no
    /// key for, which fails 2FA closed instead of letting the account in without it.
    pub totp_secret: Option<String>,
    pub totp_last_step: Option<i64>,
    /// 2FA is on, but this process cannot read the secret. Sign-in must refuse, not skip it.
    #[sqlx(default)]
    pub totp_unreadable: bool,
}

impl AccountInfo {
    /// Decrypts the sensitive columns after a read. A secret that cannot be opened is dropped
    /// with a warning: the key is missing or wrong, and guessing would weaken 2FA.
    fn opened(mut self) -> Self {
        if let Some(stored) = self.totp_secret.take() {
            self.totp_secret = bf_core::secretbox::open(&stored);
            if self.totp_secret.is_none() {
                self.totp_unreadable = true;
                tracing::error!(user = self.id.get(), "TOTP secret cannot be decrypted; check BF_SECRET_KEY");
            }
        }
        self
    }
}

macro_rules! account_cols {
    () => {
        "id, username, email, email_verified_at IS NOT NULL AS email_verified, password_hash,
    totp_secret, totp_last_step"
    };
}

pub async fn account(pool: &PgPool, user_id: UserId) -> Result<Option<AccountInfo>, Error> {
    sqlx::query_as(concat!("SELECT ", account_cols!(), " FROM users WHERE id = $1"))
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map(|a| a.map(AccountInfo::opened))
}

pub async fn account_by_email(pool: &PgPool, email: &str) -> Result<Option<AccountInfo>, Error> {
    sqlx::query_as(concat!("SELECT ", account_cols!(), " FROM users WHERE email_lower = lower($1)"))
        .bind(email)
        .fetch_optional(pool)
        .await
        .map(|a| a.map(AccountInfo::opened))
}

pub async fn mark_email_verified(pool: &PgPool, user_id: UserId) -> Result<(), Error> {
    sqlx::query("UPDATE users SET email_verified_at = coalesce(email_verified_at, now()) WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Sets a new password and signs out every other session (all of them when `keep` is `None`).
pub async fn set_password(pool: &PgPool, user_id: UserId, hash: &str, keep: Option<&str>) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1").bind(user_id).bind(hash).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_digest IS DISTINCT FROM $2")
        .bind(user_id)
        .bind(keep)
        .execute(&mut *tx)
        .await?;
    // A pending email change or an old link must not outlive the password.
    sqlx::query("UPDATE email_tokens SET used_at = now() WHERE user_id = $1 AND used_at IS NULL")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // A reset is how someone recovers from a compromise: nothing an intruder set up may survive.
    if keep.is_none() {
        for table in ["api_keys", "passkeys", "user_identities", "backup_codes"] {
            sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE user_id = $1")))
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
    }
    sqlx::query("DELETE FROM trusted_devices WHERE user_id = $1").bind(user_id).execute(&mut *tx).await?;
    // Mail keys post as this member, so they end with the old password too.
    sqlx::query("UPDATE users SET mail_post_key = NULL WHERE id = $1").bind(user_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE unsubscribe_tokens SET reply_key = NULL WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    record(&mut *tx, Entry::new("account.password").by(user_id).on(format!("user:{user_id}"))).await?;
    tx.commit().await
}

pub async fn set_totp(pool: &PgPool, user_id: UserId, secret: Option<&str>, step: Option<i64>) -> Result<(), Error> {
    // Sealed on the way in; `account()` opens it on the way out.
    let sealed = secret.map(bf_core::secretbox::seal);
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE users SET totp_secret = $2, totp_last_step = $3 WHERE id = $1")
        .bind(user_id)
        .bind(sealed)
        .bind(step)
        .execute(&mut *tx)
        .await?;
    // Backup codes belong to one 2FA setup; old ones must not work with a new secret.
    sqlx::query("DELETE FROM backup_codes WHERE user_id = $1").bind(user_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM trusted_devices WHERE user_id = $1").bind(user_id).execute(&mut *tx).await?;
    tx.commit().await
}

/// Records the accepted step so the same code cannot be used twice. Returns false when
/// another request already used this or a later step (a replay race).
pub async fn advance_totp_step(pool: &PgPool, user_id: UserId, step: i64) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE users SET totp_last_step = $2 WHERE id = $1 AND (totp_last_step IS NULL OR totp_last_step < $2)",
    )
    .bind(user_id)
    .bind(step)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

pub async fn create_pending_login(pool: &PgPool, digest: &str, user_id: UserId) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO pending_logins (token_digest, user_id, expires_at) VALUES ($1, $2, now() + interval '5 minutes')",
    )
    .bind(digest)
    .bind(user_id)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn pending_login_user(pool: &PgPool, digest: &str) -> Result<Option<UserId>, Error> {
    sqlx::query_scalar("SELECT user_id FROM pending_logins WHERE token_digest = $1 AND expires_at > now()")
        .bind(digest)
        .fetch_optional(pool)
        .await
}

/// Is this account locked out of second-factor attempts? A read for pages that only report it;
/// the code check itself goes through `begin_mfa_attempt`.
pub async fn mfa_locked(pool: &PgPool, user_id: UserId) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT coalesce(mfa_locked_until > now(), false) FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
}

/// Takes one second-factor attempt, deciding in a single statement: false when the account is
/// locked, otherwise the attempt is counted before the code is looked at (the fifth locks the
/// step for 15 minutes) and the caller may check the code. Counting first, and not after a wrong
/// answer, is what stops parallel requests all passing a separate "is it locked?" read. A right
/// code calls `clear_mfa_failures`, which hands the attempts back.
pub async fn begin_mfa_attempt(pool: &PgPool, user_id: UserId) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE users SET
           mfa_locked_until = CASE WHEN mfa_failures >= 4 THEN now() + interval '15 minutes' ELSE mfa_locked_until END,
           mfa_failures = CASE WHEN mfa_failures >= 4 THEN 0 ELSE mfa_failures + 1 END
         WHERE id = $1 AND (mfa_locked_until IS NULL OR mfa_locked_until <= now())",
    )
    .bind(user_id)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

pub async fn clear_mfa_failures(pool: &PgPool, user_id: UserId) -> Result<(), Error> {
    sqlx::query("UPDATE users SET mfa_failures = 0, mfa_locked_until = NULL WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Remembers an SSO nonce until its answer comes back.
pub async fn remember_sso_nonce(pool: &PgPool, digest: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM sso_nonces WHERE expires_at < now()").execute(pool).await?;
    sqlx::query("INSERT INTO sso_nonces (digest, expires_at) VALUES ($1, now() + interval '10 minutes')")
        .bind(digest)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Spends an SSO nonce. False if it is unknown, expired or already used.
pub async fn spend_sso_nonce(pool: &PgPool, digest: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM sso_nonces WHERE digest = $1 AND expires_at > now()")
        .bind(digest)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

pub async fn delete_pending_login(pool: &PgPool, digest: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM pending_logins WHERE token_digest = $1 OR expires_at < now()")
        .bind(digest)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn identity_user(pool: &PgPool, provider: &str, subject: &str) -> Result<Option<UserId>, Error> {
    sqlx::query_scalar("SELECT user_id FROM user_identities WHERE provider = $1 AND subject = $2")
        .bind(provider)
        .bind(subject)
        .fetch_optional(pool)
        .await
}

/// Whether the member signs in through any outside provider, so the provider's overrides apply.
pub async fn has_identity(pool: &PgPool, user_id: UserId) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM user_identities WHERE user_id = $1)")
        .bind(user_id)
        .fetch_one(pool)
        .await
}

/// The picture address the provider gave at the last login, or `None` before any was fetched.
pub async fn identity_avatar(pool: &PgPool, provider: &str, subject: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT avatar_url FROM user_identities WHERE provider = $1 AND subject = $2")
        .bind(provider)
        .bind(subject)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}

pub async fn set_identity_avatar(pool: &PgPool, provider: &str, subject: &str, url: &str) -> Result<(), Error> {
    sqlx::query("UPDATE user_identities SET avatar_url = $3 WHERE provider = $1 AND subject = $2")
        .bind(provider)
        .bind(subject)
        .bind(url)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn link_identity(
    pool: &PgPool,
    provider: &str,
    subject: &str,
    user_id: UserId,
    email: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO user_identities (provider, subject, user_id, email) VALUES ($1, $2, $3, $4)
         ON CONFLICT (provider, subject) DO NOTHING",
    )
    .bind(provider)
    .bind(subject)
    .bind(user_id)
    .bind(email)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn identities(pool: &PgPool, user_id: UserId) -> Result<Vec<(String, Option<String>)>, Error> {
    sqlx::query_as("SELECT provider, email FROM user_identities WHERE user_id = $1 ORDER BY provider")
        .bind(user_id)
        .fetch_all(pool)
        .await
}

/// Linked sign-ins with the moment each was linked, for the settings page.
pub async fn identity_rows(
    pool: &PgPool,
    user_id: UserId,
) -> Result<Vec<(String, Option<String>, DateTime<Utc>)>, Error> {
    sqlx::query_as("SELECT provider, email, created_at FROM user_identities WHERE user_id = $1 ORDER BY provider")
        .bind(user_id)
        .fetch_all(pool)
        .await
}

/// What `unlink_identity` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Unlinked {
    Done,
    /// Nothing linked under that name.
    Missing,
    /// It is the only way left to sign in (no password, passkey or other provider).
    LastMethod,
}

/// Removes one linked login, unless that would leave the account without any way to sign in.
/// The account row is locked while counting, so two unlinks at once cannot both pass.
pub async fn unlink_identity(pool: &PgPool, user_id: UserId, provider: &str) -> Result<Unlinked, Error> {
    let mut tx = pool.begin().await?;
    let has_password: bool = sqlx::query_scalar("SELECT password_hash IS NOT NULL FROM users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    let (passkeys, others, target): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM passkeys WHERE user_id = $1),
                (SELECT count(*) FROM user_identities WHERE user_id = $1 AND provider <> $2),
                (SELECT count(*) FROM user_identities WHERE user_id = $1 AND provider = $2)",
    )
    .bind(user_id)
    .bind(provider)
    .fetch_one(&mut *tx)
    .await?;
    if target == 0 {
        return Ok(Unlinked::Missing);
    }
    if !has_password && passkeys == 0 && others == 0 {
        return Ok(Unlinked::LastMethod);
    }
    sqlx::query("DELETE FROM user_identities WHERE user_id = $1 AND provider = $2")
        .bind(user_id)
        .bind(provider)
        .execute(&mut *tx)
        .await?;
    record(&mut *tx, Entry::new("account.unlink_identity").by(user_id).on(format!("user:{user_id}"))).await?;
    tx.commit().await?;
    Ok(Unlinked::Done)
}

/// How long "remember this device" skips the second step.
pub const TRUSTED_DEVICE_DAYS: i32 = 30;

/// Remembers a device (by the digest of its cookie token) for [`TRUSTED_DEVICE_DAYS`].
pub async fn trust_device(pool: &PgPool, user_id: UserId, digest: &str, user_agent: Option<&str>) -> Result<(), Error> {
    sqlx::query("DELETE FROM trusted_devices WHERE expires_at < now()").execute(pool).await?;
    sqlx::query(
        "INSERT INTO trusted_devices (token_digest, user_id, user_agent, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(days => $4))",
    )
    .bind(digest)
    .bind(user_id)
    .bind(user_agent)
    .bind(TRUSTED_DEVICE_DAYS)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn device_trusted(pool: &PgPool, user_id: UserId, digest: &str) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM trusted_devices WHERE token_digest = $1 AND user_id = $2 AND expires_at > now())",
    )
    .bind(digest)
    .bind(user_id)
    .fetch_one(pool)
    .await
}

/// Forgets every remembered device (after a password or two-factor change, or on request).
pub async fn forget_devices(pool: &PgPool, user_id: UserId) -> Result<u64, Error> {
    Ok(sqlx::query("DELETE FROM trusted_devices WHERE user_id = $1").bind(user_id).execute(pool).await?.rows_affected())
}

pub async fn trusted_device_count(pool: &PgPool, user_id: UserId) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM trusted_devices WHERE user_id = $1 AND expires_at > now()")
        .bind(user_id)
        .fetch_one(pool)
        .await
}

/// Creates an account for an external login: no password, email verified when the
/// provider vouches for it. Picks a free username from `base`.
pub async fn create_external_user(pool: &PgPool, base: &str, email: &str, verified: bool) -> Result<UserId, Error> {
    // Serialised with every other first-account check, so two empty-site registrations cannot
    // both see "no users yet" and both become administrator.
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(FIRST_USER_LOCK).execute(&mut *tx).await?;
    let (id, _) = crate::accounts::users::insert_user(
        &mut tx,
        &crate::accounts::users::NewUser {
            username: base,
            email,
            password_hash: None,
            display_name: None,
            primary_group: None,
            joined_at: None,
            verified,
            kind: crate::accounts::users::AccountKind::Member,
            fit_name: true,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// The advisory lock that orders "is this the first account?" decisions.
pub const FIRST_USER_LOCK: i64 = 0x6266_5f66_6972_7374;

pub async fn save_challenge(
    pool: &PgPool,
    digest: &str,
    challenge: &[u8],
    user_id: Option<UserId>,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO webauthn_challenges (token_digest, challenge, user_id, expires_at)
         VALUES ($1, $2, $3, now() + interval '5 minutes')",
    )
    .bind(digest)
    .bind(challenge)
    .bind(user_id)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM webauthn_challenges WHERE expires_at < now()").execute(pool).await.map(|_| ())
}

/// Takes (and deletes) a pending challenge: each one can be used once.
pub async fn take_challenge(pool: &PgPool, digest: &str) -> Result<Option<(Vec<u8>, Option<UserId>)>, Error> {
    sqlx::query_as(
        "DELETE FROM webauthn_challenges WHERE token_digest = $1 AND expires_at > now() RETURNING challenge, user_id",
    )
    .bind(digest)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Clone, FromRow)]
pub struct PasskeyRow {
    pub id: Vec<u8>,
    pub user_id: UserId,
    pub public_key: Vec<u8>,
    pub sign_count: i64,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

macro_rules! pk_cols {
    () => {
        "id, user_id, public_key, sign_count, name, created_at, last_used_at"
    };
}

pub async fn passkeys(pool: &PgPool, user_id: UserId) -> Result<Vec<PasskeyRow>, Error> {
    sqlx::query_as(concat!("SELECT ", pk_cols!(), " FROM passkeys WHERE user_id = $1 ORDER BY created_at"))
        .bind(user_id)
        .fetch_all(pool)
        .await
}

pub async fn passkey(pool: &PgPool, id: &[u8]) -> Result<Option<PasskeyRow>, Error> {
    sqlx::query_as(concat!("SELECT ", pk_cols!(), " FROM passkeys WHERE id = $1")).bind(id).fetch_optional(pool).await
}

pub async fn add_passkey(
    pool: &PgPool,
    id: &[u8],
    user_id: UserId,
    public_key: &[u8],
    sign_count: i64,
    name: &str,
) -> Result<(), Error> {
    sqlx::query("INSERT INTO passkeys (id, user_id, public_key, sign_count, name) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(user_id)
        .bind(public_key)
        .bind(sign_count)
        .bind(name)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn passkey_used(pool: &PgPool, id: &[u8], sign_count: i64) -> Result<(), Error> {
    sqlx::query("UPDATE passkeys SET sign_count = $2, last_used_at = now() WHERE id = $1")
        .bind(id)
        .bind(sign_count)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn delete_passkey(pool: &PgPool, user_id: UserId, id: &[u8]) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM passkeys WHERE user_id = $1 AND id = $2")
        .bind(user_id)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Failures from one address (a /64 for IPv6) against one account before that address is
/// refused for the account.
const FAILURES_PER_ADDRESS: i64 = 5;
/// Failures from anywhere against one account before the account itself is locked for a while.
const FAILURES_PER_ACCOUNT: i64 = 30;
/// How long both kinds of lock last. They do not double: a lock that grew to a day would let
/// anyone who knows a username keep its owner out for good.
const LOCK_MINUTES: i32 = 15;

/// The part of an address that one client controls: the address itself for IPv4, the /64 for
/// IPv6, where every home connection owns billions of addresses.
pub fn ip_bucket(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip.to_canonical() {
        std::net::IpAddr::V6(v6) => {
            let mut o = v6.octets();
            o[8..].fill(0);
            std::net::IpAddr::V6(o.into())
        }
        v4 => v4,
    }
}

fn bucket_network(ip: std::net::IpAddr) -> sqlx::types::ipnetwork::IpNetwork {
    let prefix = if ip.to_canonical().is_ipv6() { 64 } else { 32 };
    sqlx::types::ipnetwork::IpNetwork::new(ip_bucket(ip), prefix)
        .unwrap_or_else(|_| sqlx::types::ipnetwork::IpNetwork::from(ip))
}

/// Records a failed password attempt. Returns the minutes of the lock this attempt caused: for
/// the attempting address when it reaches five failures against this account, or for the whole
/// account at thirty failures from anywhere.
///
/// The lock is per account and address on purpose. A lock on the account alone would let anyone
/// who knows a username (administrators included) keep its owner out by failing on purpose; here
/// that only shuts the attacker's own address out, and the owner's address is untouched.
pub async fn record_failed_login(
    pool: &PgPool,
    login: &str,
    user_id: Option<UserId>,
    ip: Option<std::net::IpAddr>,
) -> Result<Option<i32>, Error> {
    // The login is whatever was typed; the table keeps a bounded copy.
    let login: String = login.chars().take(LOGIN_MAX_CHARS).collect();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO failed_logins (login, user_id, ip) VALUES ($1, $2, $3)")
        .bind(&login)
        .bind(user_id)
        .bind(ip.map(sqlx::types::ipnetwork::IpNetwork::from))
        .execute(&mut *tx)
        .await?;
    let mut locked = None;
    if let Some(u) = user_id {
        // Failures count from the newest of: 15 minutes ago, the last lock, the last success.
        const SINCE: &str = "greatest(now() - interval '15 minutes',
              coalesce((SELECT locked_until FROM users WHERE id = $1), '-infinity'),
              coalesce((SELECT login_cleared_at FROM users WHERE id = $1), '-infinity'))";
        let (account, address): (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT count(*), count(*) FILTER (WHERE ip <<= $2) FROM failed_logins
             WHERE user_id = $1 AND created_at > {SINCE}"
        )))
        .bind(u)
        .bind(ip.map(bucket_network))
        .fetch_one(&mut *tx)
        .await?;
        if account >= FAILURES_PER_ACCOUNT {
            sqlx::query("UPDATE users SET login_lockouts = login_lockouts + 1, locked_until = now() + make_interval(mins => $2) WHERE id = $1")
                .bind(u)
                .bind(LOCK_MINUTES)
                .execute(&mut *tx)
                .await?;
            locked = Some(LOCK_MINUTES);
        } else if address == FAILURES_PER_ADDRESS {
            locked = Some(LOCK_MINUTES);
        }
    }
    // A login that names no account is only recorded: `login_blocked_until` counts these rows.
    tx.commit().await?;
    Ok(locked)
}

/// The longest login text kept or looked up: an email address is at most 254 characters.
pub const LOGIN_MAX_CHARS: usize = 254;

/// A correct password, a reset link or a sign-in link ends the run of failures and any lock.
pub async fn clear_login_lockouts(pool: &PgPool, user_id: UserId) -> Result<(), Error> {
    sqlx::query("UPDATE users SET login_lockouts = 0, locked_until = NULL, login_cleared_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// One address trying many accounts, which no per-account lock would notice.
pub async fn ip_login_blocked(pool: &PgPool, ip: std::net::IpAddr) -> Result<bool, Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM failed_logins WHERE ip <<= $1 AND created_at > now() - interval '15 minutes'",
    )
    .bind(bucket_network(ip))
    .fetch_one(pool)
    .await?;
    Ok(n >= 50)
}

/// Until when this address may not try this login's password: the account-wide lock, or the
/// moment the address's fifth recent failure ages out, whichever is later. `user_id` is `None`
/// for a login that names no account, which is limited by the text typed instead, so the answer
/// has the same shape either way and does not show which accounts exist.
pub async fn login_blocked_until(
    pool: &PgPool,
    user_id: Option<UserId>,
    login: &str,
    ip: Option<std::net::IpAddr>,
) -> Result<Option<DateTime<Utc>>, Error> {
    login_blocked_until_for(pool, user_id, login, ip, false).await
}

/// [`login_blocked_until`] for a browser that may hold the account's "remember this device"
/// cookie (`trusted_device`). The account-wide lock is what many addresses together can cause, so
/// it does not apply to the owner: an address (or its /64) the account has signed in from before,
/// or a remembered device. The limit per address still applies to both.
pub async fn login_blocked_until_for(
    pool: &PgPool,
    user_id: Option<UserId>,
    login: &str,
    ip: Option<std::net::IpAddr>,
    trusted_device: bool,
) -> Result<Option<DateTime<Utc>>, Error> {
    sqlx::query_scalar(
        "SELECT greatest(
           (SELECT locked_until FROM users WHERE id = $1 AND locked_until > now()
              AND NOT ($6
                   OR EXISTS (SELECT 1 FROM sessions WHERE user_id = $1 AND ip <<= $2)
                   OR EXISTS (SELECT 1 FROM audit_log WHERE actor_id = $1 AND action = 'login' AND ip <<= $2))),
           (SELECT created_at + make_interval(mins => $4) FROM failed_logins
            WHERE ip <<= $2
              AND (user_id = $1 OR ($1::bigint IS NULL AND user_id IS NULL AND lower(login) = lower($5)))
              AND created_at > greatest(now() - make_interval(mins => $4),
                    coalesce((SELECT login_cleared_at FROM users WHERE id = $1), '-infinity'))
            ORDER BY created_at DESC OFFSET $3 LIMIT 1))",
    )
    .bind(user_id)
    .bind(ip.map(bucket_network))
    .bind(FAILURES_PER_ADDRESS - 1)
    .bind(LOCK_MINUTES)
    .bind(login)
    .bind(trusted_device)
    .fetch_one(pool)
    .await
}

pub async fn locked_until(pool: &PgPool, user_id: UserId) -> Result<Option<DateTime<Utc>>, Error> {
    sqlx::query_scalar("SELECT locked_until FROM users WHERE id = $1 AND locked_until > now()")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct FailedLogin {
    pub login: String,
    pub ip: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn failed_logins(pool: &PgPool, limit: i64) -> Result<Vec<FailedLogin>, Error> {
    sqlx::query_as("SELECT login, host(ip) AS ip, created_at FROM failed_logins ORDER BY id DESC LIMIT $1")
        .bind(limit)
        .fetch_all(pool)
        .await
}

/// Replaces a user's backup codes with new digests.
pub async fn set_backup_codes(pool: &PgPool, user_id: UserId, digests: &[String]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM backup_codes WHERE user_id = $1").bind(user_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO backup_codes (user_id, code_digest) SELECT $1, unnest($2::text[])")
        .bind(user_id)
        .bind(digests)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Uses a backup code once. Returns false for unknown or already used codes.
pub async fn use_backup_code(pool: &PgPool, user_id: UserId, digest: &str) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE backup_codes SET used_at = now() WHERE user_id = $1 AND code_digest = $2 AND used_at IS NULL",
    )
    .bind(user_id)
    .bind(digest)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

pub async fn backup_codes_left(pool: &PgPool, user_id: UserId) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM backup_codes WHERE user_id = $1 AND used_at IS NULL")
        .bind(user_id)
        .fetch_one(pool)
        .await
}

/// Does any of the user's groups require two-factor, and is it missing? Only an authenticator
/// (TOTP) counts: a passkey on the account says nothing about how a session was opened.
pub async fn needs_2fa(pool: &PgPool, user_id: UserId) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM groups g WHERE g.require_2fa AND (g.id = (SELECT primary_group_id FROM users WHERE id = $1)
                          OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = $1)))
            AND (SELECT totp_secret IS NULL FROM users WHERE id = $1)",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

crate::sql::set_col!(
/// Moves a member off a legacy password hash after a successful sign-in.
    upgrade_hash, "users", "password_hash", i64, &str);

/// Ends a member's mail keys (post-by-mail and reply-by-email): call when their address changes.
pub async fn reset_mail_keys(pool: &PgPool, user_id: UserId) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE users SET mail_post_key = NULL WHERE id = $1").bind(user_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE unsubscribe_tokens SET reply_key = NULL WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
