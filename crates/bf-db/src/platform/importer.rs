//! Storage for the importer, the redirect map and clubs.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

type Error = sqlx::Error;

pub async fn mapped(pool: &PgPool, kind: &str, legacy: i64) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT new_id FROM import_map WHERE kind = $1 AND legacy_id = $2")
        .bind(kind)
        .bind(legacy)
        .fetch_optional(pool)
        .await
}

pub async fn map(pool: &PgPool, kind: &str, legacy: i64, new: i64) -> Result<(), Error> {
    sqlx::query("INSERT INTO import_map (kind, legacy_id, new_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(kind)
        .bind(legacy)
        .bind(new)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Reuses an account with the same email; otherwise adds one, renaming on a name clash. Imported
/// members never become staff here, whatever order things run in.
pub async fn import_user(
    pool: &PgPool,
    username: &str,
    email: &str,
    hash: Option<&str>,
    joined: Option<DateTime<Utc>>,
) -> Result<i64, Error> {
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE email_lower = lower($1)")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    // A name that is taken, or that looks like another member's (a staff name above all), gets a
    // number; the account is never made without its skeleton.
    let mut tx = pool.begin().await?;
    let (id, _) = crate::accounts::users::insert_user(
        &mut tx,
        &crate::accounts::users::NewUser {
            username,
            email,
            password_hash: hash,
            display_name: None,
            primary_group: Some(2),
            joined_at: joined,
            verified: true,
            kind: crate::accounts::users::AccountKind::Member,
            fit_name: true,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(id.get())
}

/// A topic that was pinned (sticky, or a site-wide announcement) or closed on the old site. Only ever set, so
/// what moderators did here since is kept.
pub async fn set_topic_flags(pool: &PgPool, item: i64, pinned: bool, global: bool, locked: bool) -> Result<(), Error> {
    sqlx::query(
        "UPDATE items SET pinned = pinned OR $2 OR $3, pinned_global = pinned_global OR $3, locked = locked OR $4 WHERE id = $1",
    )
    .bind(item)
    .bind(pinned)
    .bind(global)
    .bind(locked)
    .execute(pool)
    .await
    .map(|_| ())
}

/// How many times a topic was viewed on the old site. Only ever raised, so views counted here since are kept.
pub async fn set_views(pool: &PgPool, item: i64, views: i32) -> Result<(), Error> {
    sqlx::query("UPDATE items SET view_count = greatest(view_count, $2) WHERE id = $1")
        .bind(item)
        .bind(views.max(0))
        .execute(pool)
        .await
        .map(|_| ())
}

/// When a member was last on the old site. Only ever moves forward, so a visit here since is kept.
pub async fn set_last_seen(pool: &PgPool, user: i64, at: DateTime<Utc>) -> Result<(), Error> {
    sqlx::query("UPDATE users SET last_seen_at = greatest(coalesce(last_seen_at, $2), $2) WHERE id = $1")
        .bind(user)
        .bind(at)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Where a member stood on the old site: their trust level, and the days they visited and the topics
/// they read there (credit toward the requirements of the levels here). Only ever raises, so an
/// account that already has more keeps it.
pub async fn set_standing(pool: &PgPool, user: i64, trust_level: i16, visits: i32, reads: i32) -> Result<(), Error> {
    sqlx::query(
        "UPDATE users SET trust_level = greatest(trust_level, $2), visit_credit = greatest(visit_credit, $3),
                read_credit = greatest(read_credit, $4) WHERE id = $1",
    )
    .bind(user)
    .bind(trust_level)
    .bind(visits)
    .bind(reads)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The author for posts whose member no longer exists in the source.
pub async fn import_placeholder(pool: &PgPool) -> Result<i64, Error> {
    // Dated long ago, so it is never "the newest member".
    import_user(pool, "imported", "imported@import.invalid", None, DateTime::from_timestamp(946_684_800, 0)).await
}

pub async fn set_post_time(pool: &PgPool, item: i64, number: i32, at: Option<DateTime<Utc>>) -> Result<(), Error> {
    let Some(at) = at else { return Ok(()) };
    sqlx::query("UPDATE posts SET created_at = $3 WHERE item_id = $1 AND post_number = $2")
        .bind(item)
        .bind(number)
        .bind(at)
        .execute(pool)
        .await?;
    sqlx::query(
        "UPDATE items SET last_posted_at = $2, created_at = CASE WHEN $3 = 1 THEN $2 ELSE created_at END WHERE id = $1",
    )
    .bind(item)
    .bind(at)
    .bind(number)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Puts member post counts right after posts were added in bulk.
pub async fn refresh_counters(pool: &PgPool) -> Result<(), Error> {
    sqlx::query(
        "UPDATE users u SET post_count = c.n FROM
           (SELECT p.author_id, count(*)::int AS n FROM posts p JOIN items i ON i.id = p.item_id JOIN nodes n ON n.id = i.node_id
            WHERE p.state = 'visible' AND p.action IS NULL AND i.state = 'visible' AND n.counts_posts GROUP BY p.author_id) c
         WHERE c.author_id = u.id AND u.post_count <> c.n",
    )
    .execute(pool)
    .await
    .map(|_| ())
}

/// The `posts.id` of a topic's numbered post, for things that hang on a post (reactions).
pub async fn post_row(pool: &PgPool, item: i64, number: i32) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = $2")
        .bind(item)
        .bind(number)
        .fetch_optional(pool)
        .await
}

/// Adds tags to a topic, creating the ones that do not exist yet. Tags already on it stay.
pub async fn add_tags(pool: &PgPool, item: i64, tags: &[(String, String)]) -> Result<(), Error> {
    for (name, slug) in tags {
        sqlx::query("INSERT INTO tags (name, slug) VALUES ($1, $2) ON CONFLICT (slug) DO NOTHING")
            .bind(name)
            .bind(slug)
            .execute(pool)
            .await?;
        sqlx::query(
            "INSERT INTO item_tags (item_id, tag_id) SELECT $1, id FROM tags WHERE slug = $2 ON CONFLICT DO NOTHING",
        )
        .bind(item)
        .bind(slug)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// A member group (never a staff one); an existing group of that name is reused.
pub async fn import_group(pool: &PgPool, name: &str, slug: &str) -> Result<i64, Error> {
    if let Some(id) = sqlx::query_scalar("SELECT id FROM groups WHERE slug = $1 OR lower(name) = lower($2)")
        .bind(slug)
        .bind(name)
        .fetch_optional(pool)
        .await?
    {
        return Ok(id);
    }
    sqlx::query_scalar("INSERT INTO groups (name, slug) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(slug)
        .fetch_one(pool)
        .await
}

/// Puts members in a group, unless it is one of the built-in (staff) ones.
pub async fn add_group_members(pool: &PgPool, group: i64, users: &[i64]) -> Result<(), Error> {
    if group < 100 {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    crate::community::groups::add_members(&mut tx, users, group, crate::community::groups::Grant::Automatic).await?;
    tx.commit().await
}

/// A poll with its votes. `votes` are (member, option index, rank of a ranked ballot, 1 first).
/// None when the topic has too many polls.
pub async fn import_poll(
    pool: &PgPool,
    item: i64,
    new: &crate::community::polls::NewPoll<'_>,
    votes: &[(i64, usize, Option<i16>)],
) -> Result<Option<i64>, Error> {
    let Some(poll) = crate::community::polls::add_poll(pool, item, new).await? else { return Ok(None) };
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM poll_options WHERE poll_id = $1 ORDER BY position")
        .bind(poll)
        .fetch_all(pool)
        .await?;
    for (user, index, rank) in votes {
        if let Some(option) = ids.get(*index) {
            sqlx::query(
                "INSERT INTO poll_votes (item_id, poll_id, user_id, option_id, rank) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT DO NOTHING",
            )
            .bind(item)
            .bind(poll)
            .bind(user)
            .bind(option)
            .bind(if new.kind == "ranked" { *rank } else { None })
            .execute(pool)
            .await?;
        }
    }
    Ok(Some(poll))
}

/// A private conversation: members, then messages (author, raw, cooked, time) oldest first. Every
/// participant has it marked read, as old mail should not arrive as news.
pub async fn import_conversation(
    pool: &PgPool,
    subject: &str,
    participants: &[i64],
    messages: &[(i64, String, String, Option<DateTime<Utc>>)],
) -> Result<Option<i64>, Error> {
    let Some(first) = messages.first() else { return Ok(None) };
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar("INSERT INTO conversations (subject, starter_id) VALUES ($1, $2) RETURNING id")
        .bind(subject)
        .bind(first.0)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO conversation_participants (conversation_id, user_id)
         SELECT $1, u FROM unnest($2::bigint[]) u ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(participants)
    .execute(&mut *tx)
    .await?;
    let mut last = (0_i64, None);
    for (author, raw, cooked, at) in messages {
        let m: i64 = sqlx::query_scalar(
            "INSERT INTO conversation_messages (conversation_id, author_id, raw, cooked, created_at)
             VALUES ($1, $2, $3, $4, coalesce($5, now())) RETURNING id",
        )
        .bind(id)
        .bind(author)
        .bind(raw)
        .bind(cooked)
        .bind(at)
        .fetch_one(&mut *tx)
        .await?;
        last = (m, *at);
    }
    sqlx::query("UPDATE conversations SET last_message_id = $2, last_message_at = coalesce($3, now()) WHERE id = $1")
        .bind(id)
        .bind(last.0)
        .bind(last.1)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE conversation_participants SET last_read_id = $2 WHERE conversation_id = $1")
        .bind(id)
        .bind(last.0)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(id))
}

pub(crate) const CLUB_COLS: &str = "SELECT c.id, c.name, c.node_id, n.slug, c.group_id, c.owner_id, c.open, c.archived,
    (SELECT count(*) FROM user_secondary_groups m WHERE m.group_id = c.group_id) AS members,
    EXISTS (SELECT 1 FROM user_secondary_groups m WHERE m.group_id = c.group_id AND m.user_id = $1) AS joined,
    o.username AS owner, n.description_html, n.description_md, n.topic_count, n.post_count, n.last_activity_at,
    EXISTS (SELECT 1 FROM group_requests r WHERE r.group_id = c.group_id AND r.user_id = $1) AS requested,
    EXISTS (SELECT 1 FROM club_invites i WHERE i.club_id = c.id AND i.user_id = $1
            AND i.created_at > now() - interval '30 days') AS invited
    FROM clubs c JOIN nodes n ON n.id = c.node_id LEFT JOIN users o ON o.id = c.owner_id";
