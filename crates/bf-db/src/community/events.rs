//! Calendar events: times in a time zone, repeats, RSVPs and reminders.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

pub const RECURRENCES: [&str; 5] = ["none", "daily", "weekly", "monthly", "yearly"];

pub struct NewEvent<'a> {
    pub date: NaiveDate,
    /// `HH:MM` local time, or None for an all-day event.
    pub start_time: Option<&'a str>,
    pub end_time: Option<&'a str>,
    /// An IANA zone name, e.g. `Europe/Warsaw`.
    pub timezone: &'a str,
    pub location: Option<&'a str>,
    pub recurrence: &'a str,
    pub repeat_until: Option<NaiveDate>,
    pub capacity: Option<i32>,
}

#[derive(Debug, Clone, FromRow)]
pub struct Event {
    pub item_id: i64,
    pub on_date: NaiveDate,
    pub starts_at: DateTime<Utc>,
    pub ends_at: Option<DateTime<Utc>>,
    pub timezone: String,
    pub all_day: bool,
    pub location: Option<String>,
    pub recurrence: String,
    pub repeat_until: Option<NaiveDate>,
    pub capacity: Option<i32>,
    /// `YYYY-MM-DD HH:MM` in the event's own zone.
    pub local_start: String,
    pub local_end: Option<String>,
    pub going: i64,
    pub maybe: i64,
}

/// Is this a time zone Postgres knows?
pub async fn timezone_known(pool: &PgPool, tz: &str) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)").bind(tz).fetch_one(pool).await
}

/// Sets a topic's event. Times are read in the event's zone, so 18:00 in Warsaw stays 18:00
/// there across daylight saving. A changed event is reminded about again.
/// False, and nothing changed, when the new capacity is below the number already going.
pub async fn set_event(pool: &PgPool, item: i64, e: &NewEvent<'_>) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    // Serialises with `rsvp`, which takes the same row lock.
    sqlx::query("SELECT 1 FROM topic_events WHERE item_id = $1 FOR UPDATE").bind(item).execute(&mut *tx).await?;
    if let Some(cap) = e.capacity {
        let going: i64 = sqlx::query_scalar("SELECT count(*) FROM event_rsvps WHERE item_id = $1 AND status = 'going'")
            .bind(item)
            .fetch_one(&mut *tx)
            .await?;
        if going > i64::from(cap) {
            return Ok(false);
        }
    }
    sqlx::query(
        "INSERT INTO topic_events (item_id, on_date, starts_at, ends_at, timezone, all_day, location, recurrence,
                                   repeat_until, capacity)
         SELECT $1, $2, s.at, e.at, $5, $3::text IS NULL, $6, $7, $8, $9
         FROM (SELECT (($2::date + coalesce($3::text, '00:00')::time) AT TIME ZONE $5) AS at) s,
              (SELECT CASE WHEN $4::text IS NULL THEN NULL
                           ELSE ($2::date + $4::text::time
                                 + CASE WHEN $3::text IS NOT NULL AND $4::text::time < $3::text::time
                                        THEN interval '1 day' ELSE interval '0' END) AT TIME ZONE $5 END AS at) e
         ON CONFLICT (item_id) DO UPDATE SET on_date = EXCLUDED.on_date, starts_at = EXCLUDED.starts_at,
           ends_at = EXCLUDED.ends_at, timezone = EXCLUDED.timezone, all_day = EXCLUDED.all_day,
           location = EXCLUDED.location, recurrence = EXCLUDED.recurrence, repeat_until = EXCLUDED.repeat_until,
           capacity = EXCLUDED.capacity, reminded_at = NULL",
    )
    .bind(item)
    .bind(e.date)
    .bind(e.start_time)
    .bind(e.end_time)
    .bind(e.timezone)
    .bind(e.location)
    .bind(e.recurrence)
    .bind(e.repeat_until)
    .bind(e.capacity)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn remove_event(pool: &PgPool, item: i64) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM topic_events WHERE item_id = $1").bind(item).execute(pool).await?.rows_affected() > 0)
}

macro_rules! event_cols {
    () => {
        "e.item_id, e.on_date, e.starts_at, e.ends_at, e.timezone, e.all_day, e.location, e.recurrence,
    e.repeat_until, e.capacity,
    to_char(e.starts_at AT TIME ZONE e.timezone, 'YYYY-MM-DD HH24:MI') AS local_start,
    to_char(e.ends_at AT TIME ZONE e.timezone, 'YYYY-MM-DD HH24:MI') AS local_end,
    (SELECT count(*) FROM event_rsvps r WHERE r.item_id = e.item_id AND r.status = 'going') AS going,
    (SELECT count(*) FROM event_rsvps r WHERE r.item_id = e.item_id AND r.status = 'maybe') AS maybe"
    };
}

pub async fn event(pool: &PgPool, item: i64) -> Result<Option<Event>, Error> {
    // Only the constant EVENT_COLS is interpolated; every value is bound.
    sqlx::query_as(concat!("SELECT ", event_cols!(), " FROM topic_events e WHERE e.item_id = $1"))
        .bind(item)
        .fetch_optional(pool)
        .await
}

/// Coming events in forums the viewer can read, with their topics: (event, topic slug, title).
pub async fn events(pool: &PgPool, nodes: &[i64]) -> Result<Vec<(Event, String, String)>, Error> {
    // Only the constant EVENT_COLS is interpolated; every value is bound.
    let rows: Vec<EventWithTopic> = sqlx::query_as(concat!("SELECT ", event_cols!(), ", i.slug, i.title FROM topic_events e JOIN items i ON i.id = e.item_id
         WHERE e.on_date >= CURRENT_DATE - 1 AND coalesce(e.ends_at, e.starts_at + interval '1 day') >= now()
           AND i.state = 'visible' AND i.listed AND i.node_id = ANY($1)
         ORDER BY e.starts_at, i.id LIMIT 200"))
    .bind(nodes)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.event, r.slug, r.title)).collect())
}

/// Events starting between two dates (inclusive, in their own zone) in readable forums, for a
/// month grid: (event, topic slug, title).
pub async fn events_between(
    pool: &PgPool,
    nodes: &[i64],
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<(Event, String, String)>, Error> {
    // Only the constant EVENT_COLS is interpolated; every value is bound.
    let rows: Vec<EventWithTopic> = sqlx::query_as(concat!("SELECT ", event_cols!(), ", i.slug, i.title FROM topic_events e JOIN items i ON i.id = e.item_id
         WHERE e.on_date BETWEEN $2 AND $3 AND i.state = 'visible' AND i.listed AND i.node_id = ANY($1)
         ORDER BY e.starts_at, i.id LIMIT 500"))
    .bind(nodes)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.event, r.slug, r.title)).collect())
}

/// Members who said they are going or maybe going.
pub async fn rsvp_users(pool: &PgPool, item: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT user_id FROM event_rsvps WHERE item_id = $1 AND status IN ('going', 'maybe')")
        .bind(item)
        .fetch_all(pool)
        .await
}

/// Each member's local time for `at`, in their own zone: (member, `YYYY-MM-DD HH24:MI`, zone).
/// A member with no zone, or one Postgres does not know, is given UTC.
pub async fn local_times(pool: &PgPool, ids: &[i64], at: DateTime<Utc>) -> Result<Vec<(i64, String, String)>, Error> {
    sqlx::query_as(
        "SELECT u.id, to_char($2::timestamptz AT TIME ZONE z.name, 'YYYY-MM-DD HH24:MI'), z.name
         FROM users u
         CROSS JOIN LATERAL (SELECT coalesce((SELECT name FROM pg_timezone_names WHERE name = u.timezone LIMIT 1), 'UTC') AS name) z
         WHERE u.id = ANY($1)",
    )
    .bind(ids)
    .bind(at)
    .fetch_all(pool)
    .await
}

#[derive(FromRow)]
struct EventWithTopic {
    #[sqlx(flatten)]
    event: Event,
    slug: String,
    title: String,
}

/// What became of an RSVP.
#[derive(Debug, PartialEq, Eq)]
pub enum Rsvp {
    Saved,
    Full,
    NoEvent,
}

/// RSVP: `going`, `maybe`, `not_going`, or None to take it back.
pub async fn rsvp(pool: &PgPool, item: i64, user: i64, status: Option<&str>) -> Result<Rsvp, Error> {
    let Some(status) = status else {
        sqlx::query("DELETE FROM event_rsvps WHERE item_id = $1 AND user_id = $2")
            .bind(item)
            .bind(user)
            .execute(pool)
            .await?;
        return Ok(Rsvp::Saved);
    };
    let mut tx = pool.begin().await?;
    // Serialises RSVPs on one event, so the capacity cannot be overrun by two at once.
    let capacity: Option<Option<i32>> =
        sqlx::query_scalar("SELECT capacity FROM topic_events WHERE item_id = $1 FOR UPDATE")
            .bind(item)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(capacity) = capacity else { return Ok(Rsvp::NoEvent) };
    if let (Some(cap), "going") = (capacity, status) {
        let going: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM event_rsvps WHERE item_id = $1 AND status = 'going' AND user_id <> $2",
        )
        .bind(item)
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
        if going >= i64::from(cap) {
            return Ok(Rsvp::Full);
        }
    }
    sqlx::query(
        "INSERT INTO event_rsvps (item_id, user_id, status) VALUES ($1, $2, $3)
         ON CONFLICT (item_id, user_id) DO UPDATE SET status = EXCLUDED.status",
    )
    .bind(item)
    .bind(user)
    .bind(status)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Rsvp::Saved)
}

/// This member's RSVP, if any.
pub async fn my_rsvp(pool: &PgPool, item: i64, user: i64) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT status FROM event_rsvps WHERE item_id = $1 AND user_id = $2")
        .bind(item)
        .bind(user)
        .fetch_optional(pool)
        .await
}

/// Who is going, first come first: up to `limit` names.
pub async fn going(pool: &PgPool, item: i64, limit: i64) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(
        "SELECT u.username FROM event_rsvps r JOIN users u ON u.id = r.user_id
         WHERE r.item_id = $1 AND r.status = 'going' ORDER BY r.created_at LIMIT $2",
    )
    .bind(item)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Moves repeating events whose occurrence is over on to the next one, in their own zone (so a
/// weekly 18:00 stays 18:00 across daylight saving), and reminds about it again. Events past
/// their `repeat_until` stay on their last occurrence. Returns how many moved.
pub async fn advance_repeating(pool: &PgPool) -> Result<u64, Error> {
    let mut moved = 0;
    // One step per pass; a daily event that was missed for days catches up over a few passes.
    for _ in 0..32 {
        let n = sqlx::query(
            "WITH next AS (
               SELECT item_id,
                 CASE recurrence WHEN 'daily' THEN interval '1 day' WHEN 'weekly' THEN interval '7 days'
                                 WHEN 'monthly' THEN interval '1 month' ELSE interval '1 year' END AS step
               FROM topic_events
               WHERE recurrence <> 'none' AND coalesce(ends_at, starts_at + interval '1 day') < now())
             UPDATE topic_events e SET
               starts_at = ((e.starts_at AT TIME ZONE e.timezone) + n.step) AT TIME ZONE e.timezone,
               ends_at = ((e.ends_at AT TIME ZONE e.timezone) + n.step) AT TIME ZONE e.timezone,
               on_date = (((e.starts_at AT TIME ZONE e.timezone) + n.step))::date,
               reminded_at = NULL
             FROM next n WHERE n.item_id = e.item_id
               AND coalesce(e.ends_at, e.starts_at + interval '1 day') < now()
               AND (e.repeat_until IS NULL OR (((e.starts_at AT TIME ZONE e.timezone) + n.step))::date <= e.repeat_until)",
        )
        .execute(pool)
        .await?
        .rows_affected();
        moved += n;
        if n == 0 {
            break;
        }
    }
    Ok(moved)
}

/// Events starting within a day (all-day ones, the day before), each returned once: (topic, slug,
/// title, forum, starter, watchers and everyone going or maybe going, start).
pub async fn due_event_reminders(
    pool: &PgPool,
) -> Result<Vec<(i64, String, String, i64, Vec<i64>, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "WITH due AS (
           UPDATE topic_events SET reminded_at = now()
           WHERE reminded_at IS NULL
             AND ((NOT all_day AND starts_at > now() AND starts_at <= now() + interval '24 hours')
               OR (all_day AND on_date = CURRENT_DATE + 1))
           RETURNING item_id, starts_at)
         SELECT i.id, i.slug, i.title, i.node_id,
                ARRAY(SELECT author_id FROM posts WHERE item_id = i.id AND post_number = 1 AND author_id IS NOT NULL
                      UNION SELECT user_id FROM follows WHERE target_type = 'item' AND target_id = i.id AND level = 'watch'
                      UNION SELECT user_id FROM event_rsvps WHERE item_id = i.id AND status IN ('going', 'maybe')),
                due.starts_at
         FROM due JOIN items i ON i.id = due.item_id WHERE i.state = 'visible'",
    )
    .fetch_all(pool)
    .await
}
