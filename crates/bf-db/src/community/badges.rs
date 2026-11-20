//! Storage for first-party features: badges, polls, assignment and calendar events.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Badge {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub min_posts: i32,
    #[serde(default)]
    pub min_reputation: i64,
    #[serde(default)]
    pub min_solved: i32,
    #[serde(default)]
    pub points: i32,
    /// Set on badges that come with the forum; their condition is in `SYSTEM_BADGES`.
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default = "badge_on")]
    pub enabled: bool,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub category: String,
    #[serde(default = "badge_icon")]
    pub icon: String,
    #[serde(default = "badge_color")]
    pub color: i16,
    #[serde(default)]
    pub level: i32,
}

fn badge_on() -> bool {
    true
}

fn badge_icon() -> String {
    "trophy".to_owned()
}

fn badge_color() -> i16 {
    1
}

pub async fn badges(pool: &PgPool) -> Result<Vec<Badge>, Error> {
    sqlx::query_as(
        "SELECT id, name, description, min_posts, min_reputation, min_solved, points, slug, enabled, tier, category, icon, color, level
         FROM badges ORDER BY slug IS NULL, category, points, name",
    )
    .fetch_all(pool)
    .await
}

pub async fn save_badge(pool: &PgPool, id: i64, b: &Badge) -> Result<Option<i64>, Error> {
    let sql = if id == 0 {
        "INSERT INTO badges (name, description, min_posts, min_reputation, min_solved, points, enabled, tier, category, icon, color, level)
         SELECT $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13 WHERE $1 = 0 RETURNING id"
    } else {
        "UPDATE badges SET name = $2, description = $3, min_posts = $4, min_reputation = $5, min_solved = $6, points = $7, enabled = $8,
           tier = $9, category = $10, icon = $11, color = $12, level = $13
         WHERE id = $1 AND (slug IS NULL OR slug NOT LIKE 'plugin:%') RETURNING id"
    };
    sqlx::query_scalar(sql)
        .bind(id)
        .bind(&b.name)
        .bind(&b.description)
        .bind(b.min_posts)
        .bind(b.min_reputation)
        .bind(b.min_solved)
        .bind(b.points)
        .bind(b.enabled)
        .bind(&b.tier)
        .bind(&b.category)
        .bind(&b.icon)
        .bind(b.color)
        .bind(b.level)
        .fetch_optional(pool)
        .await
}

/// Deletes a badge, the forum's own included (members who earned it lose it). A plugin's badges
/// are the plugin's: they go when it does.
pub async fn delete_badge(pool: &PgPool, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM badges WHERE id = $1 AND (slug IS NULL OR slug NOT LIKE 'plugin:%')")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Awards badges members now qualify for. Each is kept once earned. Plugin accounts earn nothing.
pub async fn award_badges(pool: &PgPool) -> Result<Vec<(i64, String)>, Error> {
    sqlx::query_as(
        "INSERT INTO user_badges (user_id, badge_id)
         SELECT u.id, b.id FROM badges b JOIN users u
           ON u.post_count >= b.min_posts AND u.reputation >= b.min_reputation
          AND (b.min_solved = 0 OR (SELECT count(*) FROM items i JOIN posts p ON p.item_id = i.id AND p.post_number = i.solved_post_number
                                    WHERE p.author_id = u.id AND i.state = 'visible') >= b.min_solved)
         WHERE b.enabled AND (b.min_posts > 0 OR b.min_reputation > 0 OR b.min_solved > 0)
           AND u.email_lower NOT LIKE '%.invalid'
         ON CONFLICT DO NOTHING
         RETURNING user_id, (SELECT name FROM badges WHERE id = badge_id)",
    )
    .fetch_all(pool)
    .await
}

/// A member's badges: (name, description), and their points.
pub async fn badges_of(
    pool: &PgPool,
    user: i64,
) -> Result<(Vec<(String, String, String, String, i16, i32)>, i64), Error> {
    let list = sqlx::query_as(
        "SELECT b.name, b.description, b.tier, b.icon, b.color, b.level FROM user_badges ub JOIN badges b ON b.id = ub.badge_id WHERE ub.user_id = $1 ORDER BY ub.earned_at",
    )
    .bind(user)
    .fetch_all(pool)
    .await?;
    let points: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(b.points), 0)::bigint FROM user_badges ub JOIN badges b ON b.id = ub.badge_id WHERE ub.user_id = $1",
    )
    .bind(user)
    .fetch_one(pool)
    .await?;
    Ok((list, points))
}

/// Turns a query for qualifying members into the award, as one fixed statement. Plugin accounts
/// earn nothing, and a switched-off badge is skipped.
macro_rules! grant {
    ($sql:expr_2021) => {
        concat!(
            "INSERT INTO user_badges (user_id, badge_id)
             SELECT DISTINCT e.user_id, b.id FROM (",
            $sql,
            ") e
             JOIN users u ON u.id = e.user_id JOIN badges b ON b.slug = $1 AND b.enabled
             WHERE u.email_lower NOT LIKE '%.invalid'
             ON CONFLICT DO NOTHING RETURNING user_id"
        )
    };
}

/// The conditions behind the badges seeded by the baseline schema.
const SYSTEM_BADGES: &[(&str, &str)] = &[
    ("basic", grant!("SELECT id AS user_id FROM users WHERE trust_level >= 1")),
    ("member", grant!("SELECT id AS user_id FROM users WHERE trust_level >= 2")),
    ("regular", grant!("SELECT id AS user_id FROM users WHERE trust_level >= 3")),
    ("leader", grant!("SELECT id AS user_id FROM users WHERE trust_level >= 4")),
    (
        "autobiographer",
        grant!("SELECT id AS user_id FROM users WHERE avatar_upload_id IS NOT NULL AND coalesce(about_md, '') <> ''"),
    ),
    ("first_like", grant!("SELECT user_id FROM post_likes GROUP BY user_id HAVING count(*) >= 1")),
    (
        "first_link",
        grant!(
            "SELECT p.author_id AS user_id FROM post_links k JOIN posts p ON p.id = k.post_id WHERE p.author_id IS NOT NULL"
        ),
    ),
    (
        "first_quote",
        grant!(
            "SELECT author_id AS user_id FROM posts WHERE author_id IS NOT NULL AND state = 'visible' AND action IS NULL AND raw ~* '(^|\\n)(> — @|\\[quote)'"
        ),
    ),
    (
        "first_mention",
        grant!(
            "SELECT author_id AS user_id FROM posts WHERE author_id IS NOT NULL AND state = 'visible' AND action IS NULL AND raw ~ '(^|\\s)@[A-Za-z0-9_]'"
        ),
    ),
    (
        "first_flag",
        grant!("SELECT reporter_id AS user_id FROM reports WHERE status = 'resolved' AND reporter_id IS NOT NULL"),
    ),
    (
        "editor",
        grant!(
            "SELECT r.editor_id AS user_id FROM post_revisions r JOIN posts p ON p.id = r.post_id WHERE r.editor_id = p.author_id"
        ),
    ),
    (
        "wiki_editor",
        grant!(
            "SELECT r.editor_id AS user_id FROM post_revisions r JOIN posts p ON p.id = r.post_id WHERE p.wiki AND r.editor_id IS NOT NULL AND r.editor_id <> p.author_id"
        ),
    ),
    (
        "welcome",
        grant!(
            "SELECT p.author_id AS user_id FROM post_likes l JOIN posts p ON p.id = l.post_id WHERE p.author_id IS NOT NULL"
        ),
    ),
    (
        "nice_post",
        grant!(
            "SELECT p.author_id AS user_id FROM posts p JOIN post_likes l ON l.post_id = p.id WHERE p.state = 'visible' AND p.author_id IS NOT NULL AND p.post_number > 1 GROUP BY p.id, p.author_id HAVING count(*) >= 10"
        ),
    ),
    (
        "good_post",
        grant!(
            "SELECT p.author_id AS user_id FROM posts p JOIN post_likes l ON l.post_id = p.id WHERE p.state = 'visible' AND p.author_id IS NOT NULL AND p.post_number > 1 GROUP BY p.id, p.author_id HAVING count(*) >= 25"
        ),
    ),
    (
        "great_post",
        grant!(
            "SELECT p.author_id AS user_id FROM posts p JOIN post_likes l ON l.post_id = p.id WHERE p.state = 'visible' AND p.author_id IS NOT NULL AND p.post_number > 1 GROUP BY p.id, p.author_id HAVING count(*) >= 50"
        ),
    ),
    (
        "nice_topic",
        grant!(
            "SELECT p.author_id AS user_id FROM posts p JOIN post_likes l ON l.post_id = p.id WHERE p.state = 'visible' AND p.author_id IS NOT NULL AND p.post_number = 1 GROUP BY p.id, p.author_id HAVING count(*) >= 10"
        ),
    ),
    (
        "good_topic",
        grant!(
            "SELECT p.author_id AS user_id FROM posts p JOIN post_likes l ON l.post_id = p.id WHERE p.state = 'visible' AND p.author_id IS NOT NULL AND p.post_number = 1 GROUP BY p.id, p.author_id HAVING count(*) >= 25"
        ),
    ),
    (
        "great_topic",
        grant!(
            "SELECT p.author_id AS user_id FROM posts p JOIN post_likes l ON l.post_id = p.id WHERE p.state = 'visible' AND p.author_id IS NOT NULL AND p.post_number = 1 GROUP BY p.id, p.author_id HAVING count(*) >= 50"
        ),
    ),
    (
        "appreciated",
        grant!(
            "SELECT author_id AS user_id FROM (SELECT p.id, p.author_id FROM posts p JOIN post_likes l ON l.post_id = p.id WHERE p.author_id IS NOT NULL AND p.state = 'visible' GROUP BY p.id, p.author_id HAVING count(*) >= 1) x GROUP BY author_id HAVING count(*) >= 20"
        ),
    ),
    (
        "respected",
        grant!(
            "SELECT author_id AS user_id FROM (SELECT p.id, p.author_id FROM posts p JOIN post_likes l ON l.post_id = p.id WHERE p.author_id IS NOT NULL AND p.state = 'visible' GROUP BY p.id, p.author_id HAVING count(*) >= 2) x GROUP BY author_id HAVING count(*) >= 100"
        ),
    ),
    (
        "admired",
        grant!(
            "SELECT author_id AS user_id FROM (SELECT p.id, p.author_id FROM posts p JOIN post_likes l ON l.post_id = p.id WHERE p.author_id IS NOT NULL AND p.state = 'visible' GROUP BY p.id, p.author_id HAVING count(*) >= 5) x GROUP BY author_id HAVING count(*) >= 300"
        ),
    ),
    ("thank_you", grant!("SELECT user_id FROM post_likes GROUP BY user_id HAVING count(*) >= 20")),
    ("gives_back", grant!("SELECT user_id FROM post_likes GROUP BY user_id HAVING count(*) >= 100")),
    ("empathetic", grant!("SELECT user_id FROM post_likes GROUP BY user_id HAVING count(*) >= 500")),
    (
        "anniversary",
        grant!(
            "SELECT u.id AS user_id FROM users u WHERE u.joined_at < now() - interval '1 year' AND EXISTS (SELECT 1 FROM posts p WHERE p.author_id = u.id AND p.created_at > now() - interval '1 year')"
        ),
    ),
    (
        "solved",
        grant!(
            "SELECT p.author_id AS user_id FROM items i JOIN posts p ON p.item_id = i.id AND p.post_number = i.solved_post_number WHERE i.state = 'visible' AND p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 1"
        ),
    ),
    (
        "tech_support",
        grant!(
            "SELECT p.author_id AS user_id FROM items i JOIN posts p ON p.item_id = i.id AND p.post_number = i.solved_post_number WHERE i.state = 'visible' AND p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 10"
        ),
    ),
    (
        "tech_guru",
        grant!(
            "SELECT p.author_id AS user_id FROM items i JOIN posts p ON p.item_id = i.id AND p.post_number = i.solved_post_number WHERE i.state = 'visible' AND p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 50"
        ),
    ),
    (
        "promoter",
        grant!(
            "SELECT i.created_by AS user_id FROM invites i JOIN users u ON u.id = i.used_by WHERE i.created_by IS NOT NULL AND u.trust_level >= 0 GROUP BY i.created_by HAVING count(*) >= 1"
        ),
    ),
    (
        "campaigner",
        grant!(
            "SELECT i.created_by AS user_id FROM invites i JOIN users u ON u.id = i.used_by WHERE i.created_by IS NOT NULL AND u.trust_level >= 1 GROUP BY i.created_by HAVING count(*) >= 3"
        ),
    ),
    (
        "champion",
        grant!(
            "SELECT i.created_by AS user_id FROM invites i JOIN users u ON u.id = i.used_by WHERE i.created_by IS NOT NULL AND u.trust_level >= 2 GROUP BY i.created_by HAVING count(*) >= 5"
        ),
    ),
    ("posts_10", grant!("SELECT id AS user_id FROM users WHERE post_count >= 10")),
    ("posts_50", grant!("SELECT id AS user_id FROM users WHERE post_count >= 50")),
    ("posts_100", grant!("SELECT id AS user_id FROM users WHERE post_count >= 100")),
    ("posts_500", grant!("SELECT id AS user_id FROM users WHERE post_count >= 500")),
    ("posts_1000", grant!("SELECT id AS user_id FROM users WHERE post_count >= 1000")),
    ("posts_5000", grant!("SELECT id AS user_id FROM users WHERE post_count >= 5000")),
    (
        "topics_1",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' GROUP BY author_id HAVING count(*) >= 1"
        ),
    ),
    (
        "topics_10",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' GROUP BY author_id HAVING count(*) >= 10"
        ),
    ),
    (
        "topics_50",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' GROUP BY author_id HAVING count(*) >= 50"
        ),
    ),
    (
        "topics_100",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' GROUP BY author_id HAVING count(*) >= 100"
        ),
    ),
    (
        "topics_500",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' GROUP BY author_id HAVING count(*) >= 500"
        ),
    ),
    (
        "hot_25",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' AND reply_count >= 25"
        ),
    ),
    (
        "hot_50",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' AND reply_count >= 50"
        ),
    ),
    (
        "hot_100",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' AND reply_count >= 100"
        ),
    ),
    (
        "viewed_1000",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' AND view_count >= 1000"
        ),
    ),
    (
        "viewed_5000",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' AND view_count >= 5000"
        ),
    ),
    (
        "viewed_25000",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE author_id IS NOT NULL AND state = 'visible' AND view_count >= 25000"
        ),
    ),
    (
        "links_25",
        grant!(
            "SELECT p.author_id AS user_id FROM post_links k JOIN posts p ON p.id = k.post_id WHERE p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 25"
        ),
    ),
    (
        "links_100",
        grant!(
            "SELECT p.author_id AS user_id FROM post_links k JOIN posts p ON p.id = k.post_id WHERE p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 100"
        ),
    ),
    (
        "wiki_1",
        grant!(
            "SELECT author_id AS user_id FROM posts WHERE wiki AND author_id IS NOT NULL AND state = 'visible' GROUP BY author_id HAVING count(*) >= 1"
        ),
    ),
    (
        "wiki_10",
        grant!(
            "SELECT author_id AS user_id FROM posts WHERE wiki AND author_id IS NOT NULL AND state = 'visible' GROUP BY author_id HAVING count(*) >= 10"
        ),
    ),
    (
        "received_10",
        grant!(
            "SELECT p.author_id AS user_id FROM post_likes l JOIN posts p ON p.id = l.post_id WHERE p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 10"
        ),
    ),
    (
        "received_50",
        grant!(
            "SELECT p.author_id AS user_id FROM post_likes l JOIN posts p ON p.id = l.post_id WHERE p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 50"
        ),
    ),
    (
        "received_100",
        grant!(
            "SELECT p.author_id AS user_id FROM post_likes l JOIN posts p ON p.id = l.post_id WHERE p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 100"
        ),
    ),
    (
        "received_500",
        grant!(
            "SELECT p.author_id AS user_id FROM post_likes l JOIN posts p ON p.id = l.post_id WHERE p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 500"
        ),
    ),
    (
        "received_1000",
        grant!(
            "SELECT p.author_id AS user_id FROM post_likes l JOIN posts p ON p.id = l.post_id WHERE p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 1000"
        ),
    ),
    (
        "received_5000",
        grant!(
            "SELECT p.author_id AS user_id FROM post_likes l JOIN posts p ON p.id = l.post_id WHERE p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 5000"
        ),
    ),
    ("gave_1000", grant!("SELECT user_id AS user_id FROM post_likes GROUP BY user_id HAVING count(*) >= 1000")),
    ("gave_2500", grant!("SELECT user_id AS user_id FROM post_likes GROUP BY user_id HAVING count(*) >= 2500")),
    ("followers_1", grant!("SELECT target_id AS user_id FROM user_follows GROUP BY target_id HAVING count(*) >= 1")),
    ("followers_10", grant!("SELECT target_id AS user_id FROM user_follows GROUP BY target_id HAVING count(*) >= 10")),
    ("followers_50", grant!("SELECT target_id AS user_id FROM user_follows GROUP BY target_id HAVING count(*) >= 50")),
    (
        "followers_100",
        grant!("SELECT target_id AS user_id FROM user_follows GROUP BY target_id HAVING count(*) >= 100"),
    ),
    (
        "followers_500",
        grant!("SELECT target_id AS user_id FROM user_follows GROUP BY target_id HAVING count(*) >= 500"),
    ),
    ("following_10", grant!("SELECT user_id AS user_id FROM user_follows GROUP BY user_id HAVING count(*) >= 10")),
    ("following_50", grant!("SELECT user_id AS user_id FROM user_follows GROUP BY user_id HAVING count(*) >= 50")),
    ("statuses_1", grant!("SELECT user_id AS user_id FROM member_statuses GROUP BY user_id HAVING count(*) >= 1")),
    ("statuses_50", grant!("SELECT user_id AS user_id FROM member_statuses GROUP BY user_id HAVING count(*) >= 50")),
    ("statuses_500", grant!("SELECT user_id AS user_id FROM member_statuses GROUP BY user_id HAVING count(*) >= 500")),
    (
        "votes_1",
        grant!("SELECT user_id AS user_id FROM poll_votes GROUP BY user_id HAVING count(DISTINCT poll_id) >= 1"),
    ),
    (
        "votes_10",
        grant!("SELECT user_id AS user_id FROM poll_votes GROUP BY user_id HAVING count(DISTINCT poll_id) >= 10"),
    ),
    (
        "votes_100",
        grant!("SELECT user_id AS user_id FROM poll_votes GROUP BY user_id HAVING count(DISTINCT poll_id) >= 100"),
    ),
    (
        "polls_1",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE poll_question IS NOT NULL AND author_id IS NOT NULL GROUP BY author_id HAVING count(*) >= 1"
        ),
    ),
    (
        "polls_10",
        grant!(
            "SELECT author_id AS user_id FROM items WHERE poll_question IS NOT NULL AND author_id IS NOT NULL GROUP BY author_id HAVING count(*) >= 10"
        ),
    ),
    (
        "events_1",
        grant!(
            "SELECT i.author_id AS user_id FROM topic_events e JOIN items i ON i.id = e.item_id WHERE i.author_id IS NOT NULL GROUP BY i.author_id HAVING count(*) >= 1"
        ),
    ),
    (
        "events_10",
        grant!(
            "SELECT i.author_id AS user_id FROM topic_events e JOIN items i ON i.id = e.item_id WHERE i.author_id IS NOT NULL GROUP BY i.author_id HAVING count(*) >= 10"
        ),
    ),
    ("bookmarks_1", grant!("SELECT user_id AS user_id FROM bookmarks GROUP BY user_id HAVING count(*) >= 1")),
    ("bookmarks_50", grant!("SELECT user_id AS user_id FROM bookmarks GROUP BY user_id HAVING count(*) >= 50")),
    (
        "guardian_5",
        grant!(
            "SELECT reporter_id AS user_id FROM reports WHERE status = 'resolved' AND reporter_id IS NOT NULL GROUP BY reporter_id HAVING count(*) >= 5"
        ),
    ),
    (
        "guardian_25",
        grant!(
            "SELECT reporter_id AS user_id FROM reports WHERE status = 'resolved' AND reporter_id IS NOT NULL GROUP BY reporter_id HAVING count(*) >= 25"
        ),
    ),
    (
        "guardian_100",
        grant!(
            "SELECT reporter_id AS user_id FROM reports WHERE status = 'resolved' AND reporter_id IS NOT NULL GROUP BY reporter_id HAVING count(*) >= 100"
        ),
    ),
    (
        "chat_1",
        grant!(
            "SELECT user_id AS user_id FROM chat_messages WHERE user_id IS NOT NULL AND deleted_at IS NULL GROUP BY user_id HAVING count(*) >= 1"
        ),
    ),
    (
        "chat_100",
        grant!(
            "SELECT user_id AS user_id FROM chat_messages WHERE user_id IS NOT NULL AND deleted_at IS NULL GROUP BY user_id HAVING count(*) >= 100"
        ),
    ),
    (
        "chat_1000",
        grant!(
            "SELECT user_id AS user_id FROM chat_messages WHERE user_id IS NOT NULL AND deleted_at IS NULL GROUP BY user_id HAVING count(*) >= 1000"
        ),
    ),
    (
        "chat_10000",
        grant!(
            "SELECT user_id AS user_id FROM chat_messages WHERE user_id IS NOT NULL AND deleted_at IS NULL GROUP BY user_id HAVING count(*) >= 10000"
        ),
    ),
    ("tenure_7", grant!("SELECT id AS user_id FROM users WHERE joined_at < now() - interval '7 days'")),
    ("tenure_30", grant!("SELECT id AS user_id FROM users WHERE joined_at < now() - interval '30 days'")),
    ("tenure_90", grant!("SELECT id AS user_id FROM users WHERE joined_at < now() - interval '90 days'")),
    ("tenure_180", grant!("SELECT id AS user_id FROM users WHERE joined_at < now() - interval '180 days'")),
    ("tenure_730", grant!("SELECT id AS user_id FROM users WHERE joined_at < now() - interval '730 days'")),
    ("tenure_1095", grant!("SELECT id AS user_id FROM users WHERE joined_at < now() - interval '1095 days'")),
    ("tenure_1825", grant!("SELECT id AS user_id FROM users WHERE joined_at < now() - interval '1825 days'")),
    ("tenure_3650", grant!("SELECT id AS user_id FROM users WHERE joined_at < now() - interval '3650 days'")),
    (
        "answers_100",
        grant!(
            "SELECT p.author_id AS user_id FROM items i JOIN posts p ON p.item_id = i.id AND p.post_number = i.solved_post_number WHERE i.state = 'visible' AND p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 100"
        ),
    ),
    (
        "answers_250",
        grant!(
            "SELECT p.author_id AS user_id FROM items i JOIN posts p ON p.item_id = i.id AND p.post_number = i.solved_post_number WHERE i.state = 'visible' AND p.author_id IS NOT NULL GROUP BY p.author_id HAVING count(*) >= 250"
        ),
    ),
    (
        "ambassador_10",
        grant!(
            "SELECT i.created_by AS user_id FROM invites i WHERE i.used_by IS NOT NULL AND i.created_by IS NOT NULL GROUP BY i.created_by HAVING count(*) >= 10"
        ),
    ),
    (
        "ambassador_25",
        grant!(
            "SELECT i.created_by AS user_id FROM invites i WHERE i.used_by IS NOT NULL AND i.created_by IS NOT NULL GROUP BY i.created_by HAVING count(*) >= 25"
        ),
    ),
    ("decorator", grant!("SELECT id AS user_id FROM users WHERE cover_upload_id IS NOT NULL")),
    ("signed", grant!("SELECT id AS user_id FROM users WHERE coalesce(signature_md, '') <> ''")),
];

/// Awards the forum's own badges and returns who should hear about it: (member, badge name).
/// The first run of each is silent, so existing members are not sent a burst of notices.
pub async fn award_system_badges(pool: &PgPool) -> Result<Vec<(i64, String)>, Error> {
    let mut news = Vec::new();
    for (slug, sql) in SYSTEM_BADGES {
        // One slow or broken badge must not cost the others, or the notifications already won.
        let users: Vec<i64> = match sqlx::query_scalar(*sql).bind(slug).fetch_all(pool).await {
            Ok(users) => users,
            Err(e) => {
                tracing::warn!(badge = slug, error = %e, "badge award failed");
                continue;
            }
        };
        let row: Option<(String, bool)> =
            sqlx::query_as("UPDATE badges b SET backfilled = true FROM badges old WHERE b.id = old.id AND b.slug = $1 AND b.enabled RETURNING b.name, old.backfilled")
                .bind(slug)
                .fetch_optional(pool)
                .await?;
        if let Some((name, true)) = row {
            news.extend(users.into_iter().map(|u| (u, name.clone())));
        }
    }
    Ok(news)
}

/// A member's newest badges for their profile: (id, name, tier, icon, colour, level, rare).
/// Rare means fewer than one member in twenty has it, on a forum big enough for that to mean much.
pub async fn recent_badges(
    pool: &PgPool,
    user: i64,
    limit: i64,
) -> Result<Vec<(i64, String, String, String, i16, i32, bool)>, Error> {
    sqlx::query_as(
        "SELECT b.id, b.name, b.tier, b.icon, b.color, b.level,
                (SELECT count(*) FROM users) >= 20
                  AND (SELECT count(*) FROM user_badges o WHERE o.badge_id = b.id) * 20 < (SELECT count(*) FROM users)
         FROM user_badges ub JOIN badges b ON b.id = ub.badge_id
         WHERE ub.user_id = $1 ORDER BY ub.earned_at DESC, b.points DESC LIMIT $2",
    )
    .bind(user)
    .bind(limit)
    .fetch_all(pool)
    .await
}
