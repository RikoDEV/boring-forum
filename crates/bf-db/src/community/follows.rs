//! Storage for follows between members, group requests, message reports, folders,
//! do not disturb, reports and staff lookups.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

type Error = sqlx::Error;

pub async fn set_ignored(pool: &PgPool, user_id: i64, other: i64, on: bool) -> Result<(), Error> {
    let q = if on {
        "INSERT INTO ignored_users (user_id, ignored_user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
    } else {
        "DELETE FROM ignored_users WHERE user_id = $1 AND ignored_user_id = $2"
    };
    sqlx::query(q).bind(user_id).bind(other).execute(pool).await.map(|_| ())
}

/// The members this one has muted: (id, username), by name.
pub async fn ignored_list(pool: &PgPool, user_id: i64) -> Result<Vec<(i64, String)>, Error> {
    sqlx::query_as(
        "SELECT u.id, u.username FROM ignored_users i JOIN users u ON u.id = i.ignored_user_id
         WHERE i.user_id = $1 ORDER BY u.username_lower",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn ignored_ids(pool: &PgPool, user_id: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT ignored_user_id FROM ignored_users WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(pool)
        .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Watch,
    Track,
    Normal,
    Mute,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s {
            "watch" => Level::Watch,
            "track" => Level::Track,
            "normal" => Level::Normal,
            "mute" => Level::Mute,
            _ => return None,
        })
    }

    pub fn key(self) -> &'static str {
        match self {
            Level::Watch => "watch",
            Level::Track => "track",
            Level::Normal => "normal",
            Level::Mute => "mute",
        }
    }
}

/// `target_type` is 'item' or 'node'. `Normal` removes the row.
pub async fn set_follow(
    pool: &PgPool,
    user_id: i64,
    target_type: &str,
    target_id: i64,
    level: Level,
) -> Result<(), Error> {
    if level == Level::Normal {
        sqlx::query("DELETE FROM follows WHERE user_id = $1 AND target_type = $2 AND target_id = $3")
            .bind(user_id)
            .bind(target_type)
            .bind(target_id)
            .execute(pool)
            .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO follows (user_id, target_type, target_id, level) VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, target_type, target_id) DO UPDATE SET level = EXCLUDED.level",
    )
    .bind(user_id)
    .bind(target_type)
    .bind(target_id)
    .bind(level.key())
    .execute(pool)
    .await
    .map(|_| ())
}

/// Sets a level only if the user has none yet (posting auto-watches, replying auto-tracks).
pub async fn follow_if_unset(pool: &PgPool, user_id: i64, item_id: i64, level: Level) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO follows (user_id, target_type, target_id, level) VALUES ($1, 'item', $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(item_id)
    .bind(level.key())
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn follow_level(pool: &PgPool, user_id: i64, target_type: &str, target_id: i64) -> Result<Level, Error> {
    let l: Option<String> =
        sqlx::query_scalar("SELECT level FROM follows WHERE user_id = $1 AND target_type = $2 AND target_id = $3")
            .bind(user_id)
            .bind(target_type)
            .bind(target_id)
            .fetch_optional(pool)
            .await?;
    Ok(l.as_deref().and_then(Level::parse).unwrap_or(Level::Normal))
}

/// Users who should hear about a new post in `item_id`: watchers of the topic, plus
/// watchers of its forum who have not muted the topic. Excludes the author.
pub async fn watchers(pool: &PgPool, item_id: i64, node_id: i64, author_id: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT f.user_id FROM follows f
         WHERE f.level = 'watch' AND f.user_id <> $3
           AND ((f.target_type = 'item' AND f.target_id = $1)
             OR (f.target_type = 'node' AND f.target_id = $2
                 AND NOT EXISTS (SELECT 1 FROM follows m WHERE m.user_id = f.user_id AND m.target_type = 'item'
                                   AND m.target_id = $1 AND m.level = 'mute')))",
    )
    .bind(item_id)
    .bind(node_id)
    .bind(author_id)
    .fetch_all(pool)
    .await
}

/// Who follows this author, except those who muted the topic the post is in.
pub async fn followers_of(pool: &PgPool, author: i64, item: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT uf.user_id FROM user_follows uf WHERE uf.target_id = $1 AND NOT EXISTS (
           SELECT 1 FROM follows f WHERE f.user_id = uf.user_id AND f.target_type = 'item' AND f.target_id = $2 AND f.level = 'mute')",
    )
    .bind(author)
    .bind(item)
    .fetch_all(pool)
    .await
}

/// (how many follow this member, names of the newest `limit`).
pub async fn followers(pool: &PgPool, user: i64, limit: i64) -> Result<(i64, Vec<String>), Error> {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM user_follows WHERE target_id = $1),
                coalesce(ARRAY(SELECT u.username FROM user_follows f JOIN users u ON u.id = f.user_id
                               WHERE f.target_id = $1 ORDER BY f.user_id DESC LIMIT $2), '{}')",
    )
    .bind(user)
    .bind(limit)
    .fetch_one(pool)
    .await
}

pub async fn is_following(pool: &PgPool, user: i64, target: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM user_follows WHERE user_id = $1 AND target_id = $2)")
        .bind(user)
        .bind(target)
        .fetch_one(pool)
        .await
}

pub async fn toggle_hide_followers(pool: &PgPool, user: i64) -> Result<(), Error> {
    sqlx::query("UPDATE users SET hide_followers = NOT hide_followers WHERE id = $1")
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn toggle_follow_user(pool: &PgPool, user: i64, target: i64) -> Result<bool, Error> {
    let removed = sqlx::query("DELETE FROM user_follows WHERE user_id = $1 AND target_id = $2")
        .bind(user)
        .bind(target)
        .execute(pool)
        .await?
        .rows_affected()
        > 0;
    if !removed {
        sqlx::query("INSERT INTO user_follows (user_id, target_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(user)
            .bind(target)
            .execute(pool)
            .await?;
    }
    Ok(!removed)
}

/// Posts by people this member follows, in forums they can read: (url parts, author, excerpt, when).
pub async fn following_feed(
    pool: &PgPool,
    user: i64,
    nodes: &[i64],
) -> Result<Vec<(i64, String, String, i32, String, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.slug, i.title, p.post_number, u.username, p.created_at
         FROM user_follows f JOIN posts p ON p.author_id = f.target_id AND p.state = 'visible' AND p.action IS NULL
         JOIN items i ON i.id = p.item_id AND i.state = 'visible' AND i.node_id = ANY($2)
         JOIN users u ON u.id = p.author_id
         WHERE f.user_id = $1 ORDER BY p.created_at DESC LIMIT 50",
    )
    .bind(user)
    .bind(nodes)
    .fetch_all(pool)
    .await
}
