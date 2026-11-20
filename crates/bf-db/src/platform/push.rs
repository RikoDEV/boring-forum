//! Web push subscriptions and embedded comment threads.

use sqlx::PgPool;

type Error = sqlx::Error;

pub async fn add(pool: &PgPool, user: i64, endpoint: &str, p256dh: &str, auth: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO push_subscriptions (endpoint, user_id, p256dh, auth) VALUES ($1, $2, $3, $4)
         ON CONFLICT (endpoint) DO UPDATE SET user_id = EXCLUDED.user_id, p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth",
    )
    .bind(endpoint)
    .bind(user)
    .bind(p256dh)
    .bind(auth)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn count_for(pool: &PgPool, user: i64) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM push_subscriptions WHERE user_id = $1").bind(user).fetch_one(pool).await
}

pub async fn remove(pool: &PgPool, user: i64, endpoint: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM push_subscriptions WHERE user_id = $1 AND endpoint = $2")
        .bind(user)
        .bind(endpoint)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn remove_endpoint(pool: &PgPool, endpoint: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = $1").bind(endpoint).execute(pool).await.map(|_| ())
}

/// A push still to send: one notification to one device.
pub struct Due {
    pub queue_id: i64,
    pub notification_id: i64,
    pub attempts: i32,
    pub notification: crate::community::notifications::Row,
    pub locale: Option<String>,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

/// Pushes give up after this many sends.
pub const MAX_ATTEMPTS: i32 = 6;

/// Turns the next `limit` new notifications (by notification, not by device) into queue rows,
/// one per subscribed device, and moves the cursor past them in the same transaction. Members in
/// do-not-disturb are skipped: push interrupts, just as email does. Only one node does this at a
/// time; the others find the cursor locked and return 0. Returns the notifications passed.
pub async fn fan_out(pool: &PgPool, limit: i64) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let Some(after): Option<i64> =
        sqlx::query_scalar("SELECT last_id FROM push_cursor WHERE id = 1 FOR UPDATE SKIP LOCKED")
            .fetch_optional(&mut *tx)
            .await?
    else {
        return Ok(0);
    };
    let upto: Option<i64> =
        sqlx::query_scalar("SELECT max(id) FROM (SELECT id FROM notifications WHERE id > $1 ORDER BY id LIMIT $2) t")
            .bind(after)
            .bind(limit)
            .fetch_one(&mut *tx)
            .await?;
    let Some(upto) = upto else { return Ok(0) };
    sqlx::query(
        "INSERT INTO push_queue (notification_id, endpoint)
         SELECT n.id, s.endpoint FROM notifications n
         JOIN push_subscriptions s ON s.user_id = n.user_id JOIN users u ON u.id = n.user_id
         LEFT JOIN notification_prefs p ON p.user_id = n.user_id AND p.kind = n.kind
         WHERE (p.channels IS NULL OR p.channels & 4 <> 0) AND n.id > $1 AND n.id <= $2 AND (u.dnd_until IS NULL OR u.dnd_until < now())
           AND NOT in_quiet_hours(u.quiet_start, u.quiet_end, u.timezone)
         ON CONFLICT DO NOTHING",
    )
    .bind(after)
    .bind(upto)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE push_cursor SET last_id = $1 WHERE id = 1").bind(upto).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(upto - after)
}

#[derive(sqlx::FromRow)]
struct Claimed {
    id: i64,
    notification_id: i64,
    attempts: i32,
    endpoint: String,
    p256dh: String,
    auth: String,
    locale: Option<String>,
}

/// Claims up to `limit` queued pushes that are due, leasing each for five minutes: on several
/// nodes a push goes to one of them, and a crash retries it. `push_done` and `push_failed` end
/// the lease. A push to a device that now belongs to someone else, or that is a day old, is
/// dropped rather than sent.
pub async fn due(pool: &PgPool, limit: i64) -> Result<Vec<Due>, Error> {
    sqlx::query("DELETE FROM push_queue WHERE created_at < now() - interval '1 day'").execute(pool).await?;
    let rows: Vec<Claimed> = sqlx::query_as(
        "WITH claimed AS (
           UPDATE push_queue q SET next_at = now() + interval '5 minutes'
           WHERE q.id IN (SELECT q2.id FROM push_queue q2
                          JOIN push_subscriptions s ON s.endpoint = q2.endpoint
                          JOIN notifications n ON n.id = q2.notification_id AND n.user_id = s.user_id
                          WHERE q2.next_at <= now() ORDER BY q2.id LIMIT $1 FOR UPDATE OF q2 SKIP LOCKED)
           RETURNING q.id, q.notification_id, q.attempts, q.endpoint)
         SELECT c.id, c.notification_id, c.attempts, s.endpoint, s.p256dh, s.auth, u.locale FROM claimed c
         JOIN push_subscriptions s ON s.endpoint = c.endpoint JOIN users u ON u.id = s.user_id ORDER BY c.id",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for c in rows {
        match crate::community::notifications::by_id(pool, c.notification_id).await? {
            Some(notification) => {
                out.push(Due {
                    queue_id: c.id,
                    notification_id: c.notification_id,
                    attempts: c.attempts,
                    notification,
                    locale: c.locale,
                    endpoint: c.endpoint,
                    p256dh: c.p256dh,
                    auth: c.auth,
                });
            }
            None => push_done(pool, c.id).await?,
        }
    }
    Ok(out)
}

/// The push was delivered, or can never be: it leaves the queue.
pub async fn push_done(pool: &PgPool, queue_id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM push_queue WHERE id = $1").bind(queue_id).execute(pool).await.map(|_| ())
}

/// A failed send: retried after 30 s, 1 min, 2 min … or dropped once `MAX_ATTEMPTS` is reached.
pub async fn push_failed(pool: &PgPool, queue_id: i64) -> Result<(), Error> {
    sqlx::query(
        "UPDATE push_queue SET attempts = attempts + 1,
           next_at = now() + interval '30 seconds' * power(2, attempts) WHERE id = $1",
    )
    .bind(queue_id)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM push_queue WHERE id = $1 AND attempts >= $2")
        .bind(queue_id)
        .bind(MAX_ATTEMPTS)
        .execute(pool)
        .await
        .map(|_| ())
}
