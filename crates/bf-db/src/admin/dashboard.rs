//! Admin control panel queries: stats, members, groups, forum tree and permissions, audit log.

use serde::Serialize;
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Serialize, FromRow)]
pub struct Stats {
    pub users: i64,
    pub topics: i64,
    pub posts: i64,
    pub pending_reviews: i64,
    pub signups_7d: i64,
    pub posts_7d: i64,
}

pub async fn stats(pool: &PgPool) -> Result<Stats, Error> {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM users) AS users,
                (SELECT count(*) FROM items WHERE state = 'visible') AS topics,
                (SELECT count(*) FROM posts WHERE state = 'visible' AND action IS NULL) AS posts,
                (SELECT count(*) FROM reviewables WHERE status = 'pending') AS pending_reviews,
                (SELECT count(*) FROM users WHERE joined_at > now() - interval '7 days') AS signups_7d,
                (SELECT count(*) FROM posts WHERE created_at > now() - interval '7 days' AND action IS NULL) AS posts_7d",
    )
    .fetch_one(pool)
    .await
}

#[derive(Debug, Serialize, FromRow)]
pub struct DayCount {
    pub day: chrono::NaiveDate,
    pub posts: i64,
    pub topics: i64,
    pub signups: i64,
    pub likes: i64,
    pub reports: i64,
    pub failed_logins: i64,
    pub searches: i64,
    pub chats: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ForumCount {
    pub title: String,
    pub posts: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct MemberCount {
    pub username: String,
    pub posts: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct LevelCount {
    pub level: i16,
    pub members: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Totals {
    pub signups_7d: i64,
    pub topics_7d: i64,
    pub topics_prev_7d: i64,
    pub posts_7d: i64,
    pub posts_prev_7d: i64,
    pub likes_7d: i64,
    pub likes_prev_7d: i64,
    pub active_1d: i64,
    pub active_7d: i64,
    pub active_30d: i64,
    pub uploads: i64,
    pub upload_bytes: i64,
    /// Visible topics of the range that nobody has answered.
    pub unanswered: i64,
    pub topics_in_range: i64,
    pub oldest_review_hours: Option<i64>,
    pub mail_queued: i64,
    pub mail_failing: i64,
    pub failed_logins_1d: i64,
    pub suspended: i64,
    pub silenced: i64,
    pub unapproved: i64,
    pub sessions: i64,
}

#[derive(Debug, Serialize)]
pub struct Analytics {
    pub days: i32,
    #[serde(flatten)]
    pub totals: Totals,
    pub daily: Vec<DayCount>,
    pub top_forums: Vec<ForumCount>,
    pub top_members: Vec<MemberCount>,
    pub trust_levels: Vec<LevelCount>,
    /// Posts by hour of the day (0-23, UTC) over the range.
    pub by_hour: Vec<i64>,
    /// Posts by weekday (Monday first) over the range.
    pub by_weekday: Vec<i64>,
}

/// The dashboard's numbers: the last `days` days by day, and this week against the one before.
pub async fn analytics(pool: &PgPool, days: i32) -> Result<Analytics, Error> {
    let days = days.clamp(7, 90);
    let totals: Totals = sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM users WHERE joined_at > now() - interval '7 days') AS signups_7d,
           (SELECT count(*) FROM items WHERE state = 'visible' AND created_at > now() - interval '7 days') AS topics_7d,
           (SELECT count(*) FROM items WHERE state = 'visible' AND created_at <= now() - interval '7 days'
              AND created_at > now() - interval '14 days') AS topics_prev_7d,
           (SELECT count(*) FROM posts WHERE state = 'visible' AND action IS NULL AND created_at > now() - interval '7 days') AS posts_7d,
           (SELECT count(*) FROM posts WHERE state = 'visible' AND action IS NULL AND created_at <= now() - interval '7 days'
              AND created_at > now() - interval '14 days') AS posts_prev_7d,
           (SELECT count(*) FROM post_likes WHERE created_at > now() - interval '7 days') AS likes_7d,
           (SELECT count(*) FROM post_likes WHERE created_at <= now() - interval '7 days'
              AND created_at > now() - interval '14 days') AS likes_prev_7d,
           (SELECT count(*) FROM users WHERE last_seen_at > now() - interval '1 day') AS active_1d,
           (SELECT count(*) FROM users WHERE last_seen_at > now() - interval '7 days') AS active_7d,
           (SELECT count(*) FROM users WHERE last_seen_at > now() - interval '30 days') AS active_30d,
           (SELECT count(*) FROM uploads) AS uploads,
           (SELECT coalesce(sum(size), 0)::bigint FROM uploads) AS upload_bytes,
           (SELECT count(*) FROM items WHERE state = 'visible' AND reply_count = 0
              AND created_at > now() - make_interval(days => $1)) AS unanswered,
           (SELECT count(*) FROM items WHERE state = 'visible'
              AND created_at > now() - make_interval(days => $1)) AS topics_in_range,
           (SELECT (extract(epoch FROM now() - min(created_at)) / 3600)::bigint
              FROM reviewables WHERE status = 'pending') AS oldest_review_hours,
           (SELECT count(*) FROM mail_outbox WHERE sent_at IS NULL) AS mail_queued,
           (SELECT count(*) FROM mail_outbox WHERE sent_at IS NULL AND last_error IS NOT NULL) AS mail_failing,
           (SELECT count(*) FROM failed_logins WHERE created_at > now() - interval '1 day') AS failed_logins_1d,
           (SELECT count(*) FROM users WHERE suspended_until > now()) AS suspended,
           (SELECT count(*) FROM users WHERE silenced_until > now()) AS silenced,
           (SELECT count(*) FROM users WHERE NOT approved) AS unapproved,
           (SELECT count(*) FROM sessions WHERE expires_at > now()) AS sessions",
    )
    .bind(days)
    .fetch_one(pool)
    .await?;
    let daily = sqlx::query_as(
        "SELECT d::date AS day,
           (SELECT count(*) FROM posts p WHERE p.state = 'visible' AND p.action IS NULL AND p.created_at >= d AND p.created_at < d + interval '1 day') AS posts,
           (SELECT count(*) FROM items i WHERE i.state = 'visible' AND i.created_at >= d AND i.created_at < d + interval '1 day') AS topics,
           (SELECT count(*) FROM users u WHERE u.joined_at >= d AND u.joined_at < d + interval '1 day') AS signups,
           (SELECT count(*) FROM post_likes l WHERE l.created_at >= d AND l.created_at < d + interval '1 day') AS likes,
           (SELECT count(*) FROM reviewables r WHERE r.created_at >= d AND r.created_at < d + interval '1 day') AS reports,
           (SELECT count(*) FROM failed_logins f WHERE f.created_at >= d AND f.created_at < d + interval '1 day') AS failed_logins,
           (SELECT count(*) FROM search_logs s WHERE s.created_at >= d AND s.created_at < d + interval '1 day') AS searches,
           (SELECT count(*) FROM chat_messages c WHERE c.deleted_at IS NULL AND c.created_at >= d AND c.created_at < d + interval '1 day') AS chats
         FROM generate_series(date_trunc('day', now()) - make_interval(days => $1 - 1), date_trunc('day', now()), interval '1 day') d
         ORDER BY d",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    let top_forums = sqlx::query_as(
        "SELECT n.title, count(*) AS posts FROM posts p JOIN items i ON i.id = p.item_id JOIN nodes n ON n.id = i.node_id
         WHERE p.state = 'visible' AND p.action IS NULL AND i.state = 'visible' AND p.created_at > now() - make_interval(days => $1)
         GROUP BY n.id, n.title ORDER BY posts DESC, n.title LIMIT 6",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    let top_members = sqlx::query_as(
        "SELECT u.username, count(*) AS posts FROM posts p JOIN users u ON u.id = p.author_id
         WHERE p.state = 'visible' AND p.action IS NULL AND p.created_at > now() - make_interval(days => $1)
         GROUP BY u.id, u.username ORDER BY posts DESC, u.username LIMIT 6",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    let trust_levels = sqlx::query_as(
        "SELECT trust_level AS level, count(*) AS members FROM users WHERE NOT remote GROUP BY trust_level ORDER BY trust_level",
    )
    .fetch_all(pool)
    .await?;
    let hours: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT extract(hour FROM created_at)::int, count(*) FROM posts
         WHERE state = 'visible' AND action IS NULL AND created_at > now() - make_interval(days => $1) GROUP BY 1",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    let weekdays: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT (extract(isodow FROM created_at)::int - 1), count(*) FROM posts
         WHERE state = 'visible' AND action IS NULL AND created_at > now() - make_interval(days => $1) GROUP BY 1",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    let spread = |rows: Vec<(i32, i64)>, n: usize| {
        let mut out = vec![0; n];
        for (i, c) in rows {
            if let Some(slot) = out.get_mut(i as usize) {
                *slot = c;
            }
        }
        out
    };
    Ok(Analytics {
        days,
        totals,
        daily,
        top_forums,
        top_members,
        trust_levels,
        by_hour: spread(hours, 24),
        by_weekday: spread(weekdays, 7),
    })
}
