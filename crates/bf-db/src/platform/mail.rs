//! The mail outbox, bounces, bulk mail and unsubscribe tokens.

use crate::admin::audit::{Entry, record};
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

pub async fn queue_mail(pool: &PgPool, to: &str, subject: &str, body: &str) -> Result<(), Error> {
    sqlx::query("INSERT INTO mail_outbox (to_addr, subject, body) VALUES ($1, $2, $3)")
        .bind(to)
        .bind(subject)
        .bind(body)
        .execute(pool)
        .await
        .map(|_| ())
}

/// How many times a mail is tried before it is given up.
pub const MAX_ATTEMPTS: i16 = 9;

/// Minutes to wait after the first, second, … failed try: about two days in all, so a mail server
/// that is down overnight loses nothing.
pub const RETRY_MINUTES: [i32; MAX_ATTEMPTS as usize - 1] = [1, 5, 15, 60, 180, 360, 720, 1440];

/// Queues a mail from the contact form, which [`contact_mails_last_hour`] counts.
pub async fn queue_contact_mail(pool: &PgPool, to: &str, subject: &str, body: &str) -> Result<(), Error> {
    sqlx::query("INSERT INTO mail_outbox (to_addr, subject, body, purpose) VALUES ($1, $2, $3, 'contact')")
        .bind(to)
        .bind(subject)
        .bind(body)
        .execute(pool)
        .await
        .map(|_| ())
}

#[derive(Debug, Clone, FromRow)]
pub struct OutMail {
    pub id: i64,
    pub to_addr: String,
    pub subject: String,
    pub body: String,
    pub attempts: i16,
    pub unsubscribe_url: Option<String>,
    pub reply_key: Option<String>,
}

/// Unsent mail with attempts left and its retry time reached, oldest first.
pub async fn unsent_mail(pool: &PgPool, limit: i64) -> Result<Vec<OutMail>, Error> {
    // A lease, not a read: on several nodes each row goes to one sender, and a crash retries it.
    // It outlasts a whole batch of sends at their time limit (`SEND_TIMEOUT` in the sender).
    sqlx::query_as(
        "UPDATE mail_outbox m SET not_before = now() + interval '15 minutes'
         WHERE m.id IN (SELECT id FROM mail_outbox
                        WHERE sent_at IS NULL AND attempts < $2 AND (not_before IS NULL OR not_before <= now())
                        ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED)
         RETURNING m.id, m.to_addr, m.subject, m.body, m.attempts, m.unsubscribe_url, m.reply_key",
    )
    .bind(limit)
    .bind(MAX_ATTEMPTS)
    .fetch_all(pool)
    .await
}

/// Marks a message sent. A working delivery also clears the recipient's bounce count, so old
/// bounces do not add up to a bad address over the years.
pub async fn mail_sent(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query(
        "WITH s AS (UPDATE mail_outbox SET sent_at = now(), attempts = attempts + 1 WHERE id = $1 RETURNING to_addr)
         UPDATE users SET email_bounces = 0
         WHERE email_bounces > 0 AND NOT email_bad AND email_lower = lower((SELECT to_addr FROM s))",
    )
    .bind(id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Records a failed try. A `permanent` one (the server refused the mail for good) is not tried
/// again; any other waits the next of [`RETRY_MINUTES`].
pub async fn mail_failed(pool: &PgPool, id: i64, error: &str, permanent: bool) -> Result<(), Error> {
    // `attempts` on the right is the count before this try; arrays count from one. Past the last
    // wait the index finds nothing, and the mail is out of tries anyway.
    sqlx::query(
        "UPDATE mail_outbox SET last_error = $2,
           attempts = CASE WHEN $3 THEN $4 ELSE attempts + 1 END,
           not_before = now() + interval '1 minute' * coalesce(($5::int[])[attempts + 1], 0) WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .bind(permanent)
    .bind(MAX_ATTEMPTS)
    .bind(RETRY_MINUTES.as_slice())
    .execute(pool)
    .await
    .map(|_| ())
}

/// Counts a bounce for `email`; the address is marked bad at `limit`. False if unknown.
pub async fn record_bounce(pool: &PgPool, email: &str, limit: i32) -> Result<bool, Error> {
    let r = sqlx::query(
        "UPDATE users SET email_bounces = email_bounces + 1, email_bad = email_bad OR email_bounces + 1 >= $2
         WHERE lower(email) = lower($1)",
    )
    .bind(email)
    .bind(limit)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Closes queued mail to bad addresses, so it is neither sent nor retried.
pub async fn skip_mail_to_bad_addresses(pool: &PgPool) -> Result<(), Error> {
    sqlx::query(
        "UPDATE mail_outbox SET sent_at = now(), last_error = 'skipped: address marked bad'
         WHERE sent_at IS NULL AND lower(to_addr) IN (SELECT lower(email) FROM users WHERE email_bad)",
    )
    .execute(pool)
    .await?;
    // Delayed notification mail: read in the app meanwhile, so no email is needed.
    sqlx::query(
        "UPDATE mail_outbox o SET sent_at = now(), last_error = 'skipped: read before sending'
         WHERE o.sent_at IS NULL AND o.unless_read_user IS NOT NULL AND o.not_before <= now() AND EXISTS (
           SELECT 1 FROM notifications n WHERE n.user_id = o.unless_read_user AND n.read AND n.data->>'url' = o.unless_read_url
             AND n.created_at >= o.created_at - interval '1 minute')",
    )
    .execute(pool)
    .await?;
    // Held notification mail about a topic that is gone or hidden since it was queued.
    let account: Vec<&str> = bf_core::notifications::KINDS.iter().filter(|k| k.mandatory).map(|k| k.key).collect();
    sqlx::query(
        "UPDATE mail_outbox o SET sent_at = now(), last_error = 'skipped: topic no longer visible'
         WHERE o.sent_at IS NULL AND o.unless_read_user IS NOT NULL
           AND o.unless_read_url ~ '^/t/[^/]+/[0-9]{1,18}'
           AND NOT EXISTS (SELECT 1 FROM items i WHERE i.state = 'visible'
                           AND i.id = substring(o.unless_read_url from '^/t/[^/]+/([0-9]{1,18})')::bigint)
           AND NOT EXISTS (SELECT 1 FROM notifications n WHERE n.user_id = o.unless_read_user
                           AND n.data->>'url' = o.unless_read_url AND n.kind = ANY($1))",
    )
    .bind(account)
    .execute(pool)
    .await
    .map(|_| ())
}

/// (id, username, email, bounces) of accounts with a bad address.
pub async fn bad_emails(pool: &PgPool) -> Result<Vec<(i64, String, String, i32)>, Error> {
    sqlx::query_as("SELECT id, username, email, email_bounces FROM users WHERE email_bad ORDER BY id LIMIT 500")
        .fetch_all(pool)
        .await
}

pub async fn clear_bad_email(pool: &PgPool, user_id: i64) -> Result<bool, Error> {
    let r = sqlx::query("UPDATE users SET email_bad = false, email_bounces = 0 WHERE id = $1 AND email_bad")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}
/// Who a mass message goes to: members matching every filter that is set.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Audience {
    /// Members of any of these groups (primary or other); empty is everyone.
    pub groups: Vec<i64>,
    pub trust_min: Option<i16>,
    pub trust_max: Option<i16>,
    /// Seen on the site within this many days.
    pub active_days: Option<i32>,
    pub min_posts: i32,
    /// Only addresses the member has confirmed.
    pub verified_only: bool,
}

impl Default for Audience {
    fn default() -> Self {
        Self { groups: vec![], trust_min: None, trust_max: None, active_days: None, min_posts: 0, verified_only: true }
    }
}

/// The members a message can reach: approved, local, not suspended or leaving, with a working
/// address, and matching the filters.
macro_rules! audience {
    () => {
        "u.approved AND NOT u.remote AND u.delete_after IS NULL
    AND NOT u.email_bad AND u.email_lower NOT LIKE '%.invalid'
    AND (u.suspended_until IS NULL OR u.suspended_until < now())
    AND ($1::bigint[] = '{}' OR u.primary_group_id = ANY($1)
         OR EXISTS (SELECT 1 FROM user_secondary_groups g WHERE g.user_id = u.id AND g.group_id = ANY($1)))
    AND ($2::smallint IS NULL OR u.trust_level >= $2) AND ($3::smallint IS NULL OR u.trust_level <= $3)
    AND ($4::int IS NULL OR u.last_seen_at > now() - make_interval(days => $4))
    AND u.post_count >= $5 AND (NOT $6 OR u.email_verified_at IS NOT NULL)"
    };
}

/// Members a message to this audience reaches, most recent first, up to 200,000.
pub async fn audience_ids(pool: &PgPool, a: &Audience) -> Result<Vec<i64>, Error> {
    // Only the constant AUDIENCE is interpolated; every value is bound.
    sqlx::query_scalar(concat!("SELECT u.id FROM users u WHERE ", audience!(), " ORDER BY u.id LIMIT 200000"))
        .bind(&a.groups)
        .bind(a.trust_min)
        .bind(a.trust_max)
        .bind(a.active_days)
        .bind(a.min_posts)
        .bind(a.verified_only)
        .fetch_all(pool)
        .await
}

/// (members reached, of them how many also get the email: those who have not switched it off).
pub async fn audience_counts(pool: &PgPool, a: &Audience) -> Result<(i64, i64), Error> {
    sqlx::query_as(concat!(
        "SELECT count(*), count(*) FILTER (WHERE coalesce(p.channels, 3) & 2 <> 0)
         FROM users u LEFT JOIN notification_prefs p ON p.user_id = u.id AND p.kind = 'bulk'
         WHERE ",
        audience!()
    ))
    .bind(&a.groups)
    .bind(a.trust_min)
    .bind(a.trust_max)
    .bind(a.active_days)
    .bind(a.min_posts)
    .bind(a.verified_only)
    .fetch_one(pool)
    .await
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct MassMessage {
    pub id: i64,
    pub sender: Option<String>,
    pub subject: String,
    pub notice: bool,
    pub conversation: bool,
    pub recipients: i32,
    pub delivered: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub async fn start_mass_message(
    pool: &PgPool,
    sender: Option<i64>,
    subject: &str,
    body: &str,
    audience: &Audience,
    (notice, conversation): (bool, bool),
    recipients: i32,
) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO mass_messages (sender_id, subject, body, audience, notice, conversation, recipients)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(sender)
    .bind(subject)
    .bind(body)
    .bind(serde_json::to_value(audience).unwrap_or_default())
    .bind(notice)
    .bind(conversation)
    .bind(recipients)
    .fetch_one(pool)
    .await
}

pub async fn mass_message_progress(pool: &PgPool, id: i64, delivered: i32, status: &str) -> Result<(), Error> {
    sqlx::query(
        "UPDATE mass_messages SET delivered = $2, status = $3,
           finished_at = CASE WHEN $3 = 'running' THEN NULL ELSE now() END, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(delivered)
    .bind(status)
    .execute(pool)
    .await
    .map(|_| ())
}

/// A job whose delivery stopped with the server: (id, sender, subject, body, audience, notice,
/// conversation, delivered so far).
pub type StalledJob = (i64, Option<i64>, String, String, serde_json::Value, bool, bool, i32);

/// Claims the deliveries nobody has advanced for five minutes (the server stopped), so each is
/// resumed by exactly one node; the claim is itself a heartbeat.
pub async fn claim_stalled_mass_messages(pool: &PgPool) -> Result<Vec<StalledJob>, Error> {
    sqlx::query_as(
        "UPDATE mass_messages SET updated_at = now()
         WHERE status = 'running' AND updated_at < now() - interval '5 minutes'
           AND created_at > now() - interval '2 hours'
         RETURNING id, sender_id, subject, body, audience, notice, conversation, delivered",
    )
    .fetch_all(pool)
    .await
}

/// A delivery that kept stalling for two hours will not finish: say so.
pub async fn fail_stale_mass_messages(pool: &PgPool) -> Result<u64, Error> {
    Ok(sqlx::query(
        "UPDATE mass_messages SET status = 'failed', finished_at = now()
         WHERE status = 'running' AND created_at < now() - interval '2 hours'",
    )
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn mass_messages(pool: &PgPool, limit: i64) -> Result<Vec<MassMessage>, Error> {
    sqlx::query_as(
        "SELECT m.id, u.username AS sender, m.subject, m.notice, m.conversation, m.recipients, m.delivered,
                m.status, m.created_at, m.finished_at
         FROM mass_messages m LEFT JOIN users u ON u.id = m.sender_id ORDER BY m.id DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Contact form mails queued in the last hour.
pub async fn contact_mails_last_hour(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM mail_outbox WHERE purpose = 'contact' AND created_at > now() - interval '1 hour'",
    )
    .fetch_one(pool)
    .await
}

pub async fn log_bulk(pool: &PgPool, actor: Option<i64>, subject: &str, count: i64) -> Result<(), Error> {
    record(pool, Entry::new("mail.bulk").by(actor).on(subject).details(serde_json::json!({ "recipients": count })))
        .await
}

/// Creates the unsubscribe token for one email.
pub async fn unsubscribe_token(pool: &PgPool, user_id: i64, kind: &str, token: &str) -> Result<(), Error> {
    sqlx::query("INSERT INTO unsubscribe_tokens (token, user_id, kind) VALUES ($1, $2, $3)")
        .bind(token)
        .bind(user_id)
        .bind(kind)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn token_target(pool: &PgPool, token: &str) -> Result<Option<(i64, String)>, Error> {
    sqlx::query_as("SELECT user_id, kind FROM unsubscribe_tokens WHERE token = $1")
        .bind(token)
        .fetch_optional(pool)
        .await
}

/// Turns off email for one notification type (the in-app channel is kept).
pub async fn unsubscribe(pool: &PgPool, user_id: i64, kind: &str) -> Result<(), Error> {
    // The digest is a setting of the account, not a notification type.
    if kind == "digest" {
        return sqlx::query("UPDATE users SET digest = 'none' WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await
            .map(|_| ());
    }
    sqlx::query(
        "INSERT INTO notification_prefs (user_id, kind, channels) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, kind) DO UPDATE SET channels = notification_prefs.channels & ~2::smallint",
    )
    .bind(user_id)
    .bind(kind)
    // No row yet: the kind's defaults with email off, so push and the app channel stay as they were.
    .bind(bf_core::notifications::default_channels(kind) & !bf_core::notifications::EMAIL)
    .execute(pool)
    .await
    .map(|_| ())
}
