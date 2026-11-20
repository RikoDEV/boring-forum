//! Topic moderation that moves posts between topics or topics between forums
//!: move, merge, split, restore, edit history and the deletion log.

use crate::admin::audit::{Entry, record};
use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

async fn node_of(tx: &mut Transaction<'_, Postgres>, item_id: i64) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT node_id FROM items WHERE id = $1").bind(item_id).fetch_one(&mut **tx).await
}

/// Recomputes one topic's counters from its visible posts.
pub(crate) async fn recount_item(tx: &mut Transaction<'_, Postgres>, item_id: i64) -> Result<(), Error> {
    sqlx::query(
        "UPDATE items i SET
           reply_count = greatest(coalesce(s.n, 0) - 1, 0),
           last_post_id = s.last_id,
           last_posted_at = coalesce(s.last_at, i.created_at),
           last_poster_id = coalesce(s.last_author, i.author_id),
           like_count = coalesce((SELECT sum(like_count) FROM posts WHERE item_id = $1 AND state = 'visible'), 0)::int,
           -- Never lowered: a reader's marker only grows, so a number once used must not come back.
           highest_post_number = greatest(i.highest_post_number, coalesce((SELECT max(post_number) FROM posts WHERE item_id = $1), 0))
         FROM (SELECT count(*) AS n,
                 (array_agg(id ORDER BY created_at DESC, post_number DESC))[1] AS last_id,
                 max(created_at) AS last_at,
                 (array_agg(author_id ORDER BY created_at DESC, post_number DESC))[1] AS last_author
               FROM posts WHERE item_id = $1 AND state = 'visible' AND action IS NULL) s
         WHERE i.id = $1",
    )
    .bind(item_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// Recomputes `users.post_count` of everyone who has a post in this topic, with the formula used
/// everywhere else (visible post, visible topic, forum that counts posts). Call it after anything
/// that changes a topic's forum, state or posts (move, merge, split, copy).
pub async fn recount_authors_of(tx: &mut Transaction<'_, Postgres>, item_id: i64) -> Result<(), Error> {
    sqlx::query(
        "UPDATE users u SET post_count =
           (SELECT count(*) FROM posts p JOIN items i ON i.id = p.item_id JOIN nodes n ON n.id = i.node_id
            WHERE p.author_id = u.id AND p.state = 'visible' AND p.action IS NULL AND i.state = 'visible' AND n.counts_posts)
         WHERE u.id IN (SELECT DISTINCT author_id FROM posts WHERE item_id = $1 AND author_id IS NOT NULL)",
    )
    .bind(item_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// [`recount_authors_of`] for a caller that has no transaction.
pub async fn recount_authors_of_pool(pool: &PgPool, item_id: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    recount_authors_of(&mut tx, item_id).await?;
    tx.commit().await
}

/// Recomputes one forum's counters from its visible topics.
pub(crate) async fn recount_node(tx: &mut Transaction<'_, Postgres>, node_id: i64) -> Result<(), Error> {
    sqlx::query(
        "UPDATE nodes n SET topic_count = coalesce(s.topics, 0), post_count = coalesce(s.posts, 0),
           last_item_id = s.last_item, last_activity_at = s.last_at
         FROM (SELECT count(*) AS topics, sum(reply_count + 1) AS posts,
                 (array_agg(id ORDER BY last_posted_at DESC))[1] AS last_item, max(last_posted_at) AS last_at
               FROM items WHERE node_id = $1 AND state = 'visible') s
         WHERE n.id = $1",
    )
    .bind(node_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// Adds a small action entry ("moved", "merged", ...) to a topic's post flow.
/// Renames a topic and leaves an entry saying so. The address keeps its slug, so links still work.
pub async fn retitle(pool: &PgPool, item: i64, title: &str, actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE items SET title = $2, updated_at = now() WHERE id = $1")
        .bind(item)
        .bind(title)
        .execute(&mut *tx)
        .await?;
    action_entry(&mut tx, item, actor, &format!("retitled: {title}")).await?;
    tx.commit().await
}

async fn action_entry(tx: &mut Transaction<'_, Postgres>, item_id: i64, actor: i64, action: &str) -> Result<(), Error> {
    // Same lock as a reply, so a moderator action cannot take the same post number.
    sqlx::query("SELECT id FROM items WHERE id = $1 FOR UPDATE").bind(item_id).fetch_one(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO posts (item_id, author_id, post_number, raw, cooked, cooked_version, action)
         SELECT $1, $2, coalesce(max(post_number), 0) + 1, '', '', 0, $3 FROM posts WHERE item_id = $1",
    )
    .bind(item_id)
    .bind(actor)
    .bind(action)
    .execute(&mut **tx)
    .await?;
    recount_item(tx, item_id).await
}

async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    actor: impl Into<Option<i64>>,
    action: &str,
    target: String,
    details: serde_json::Value,
) -> Result<(), Error> {
    record(&mut **tx, Entry::new(action).by(actor.into()).on(target).details(details)).await
}

pub async fn move_topic(pool: &PgPool, item_id: i64, to_node: i64, actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    let from: i64 = sqlx::query_scalar("SELECT node_id FROM items WHERE id = $1 FOR UPDATE")
        .bind(item_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE items SET node_id = $2, updated_at = now() WHERE id = $1")
        .bind(item_id)
        .bind(to_node)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE reviewables SET node_id = $2 WHERE target_type = 'post' AND target_id IN (SELECT id FROM posts WHERE item_id = $1)")
        .bind(item_id)
        .bind(to_node)
        .execute(&mut *tx)
        .await?;
    action_entry(&mut tx, item_id, actor, "moved").await?;
    recount_node(&mut tx, from).await?;
    recount_node(&mut tx, to_node).await?;
    // The two forums may differ in `counts_posts`, so the authors' totals change too.
    recount_authors_of(&mut tx, item_id).await?;
    audit(&mut tx, actor, "topic.move", format!("item:{item_id}"), serde_json::json!({ "from": from, "to": to_node }))
        .await?;
    tx.commit().await
}

/// Appends `from`'s posts to `into` in time order. `from` is hidden and keeps a
/// pointer so its URL redirects ("merge, redirect kept").
pub async fn merge_topics(pool: &PgPool, from: i64, into: i64, actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    // Lock both in id order so two concurrent merges cannot deadlock.
    sqlx::query("SELECT id FROM items WHERE id = ANY($1) ORDER BY id FOR UPDATE")
        .bind([from.min(into), from.max(into)])
        .execute(&mut *tx)
        .await?;
    // Posts moved into a deleted topic would be purged along with it.
    let target_live: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM items WHERE id = $1 AND state = 'visible')")
            .bind(into)
            .fetch_one(&mut *tx)
            .await?;
    if !target_live {
        return Err(Error::RowNotFound);
    }
    // Numbers continue after the highest ever used in the target, so none is reused.
    let renumbered: Vec<(i32, i32)> = sqlx::query_as(
        "WITH base AS (SELECT greatest(coalesce(max(post_number), 0), (SELECT highest_post_number FROM items WHERE id = $2)) AS n
                       FROM posts WHERE item_id = $2),
              moved AS (SELECT id, post_number AS old_n, row_number() OVER (ORDER BY created_at, post_number) AS rn
                        FROM posts WHERE item_id = $1)
         UPDATE posts p SET item_id = $2, post_number = base.n + moved.rn
         FROM moved, base WHERE p.id = moved.id RETURNING moved.old_n, p.post_number",
    )
    .bind(from)
    .bind(into)
    .fetch_all(&mut *tx)
    .await?;
    let (old_numbers, new_numbers): (Vec<i32>, Vec<i32>) = renumbered.into_iter().unzip();
    move_topic_data(&mut tx, from, into, &old_numbers, &new_numbers).await?;
    sqlx::query(
        "UPDATE items SET state = 'deleted', deleted_at = now(), deleted_by = $3,
           meta = meta || jsonb_build_object('merged_into', $2::bigint) WHERE id = $1",
    )
    .bind(from)
    .bind(into)
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    let (from_node, into_node) = (node_of(&mut tx, from).await?, node_of(&mut tx, into).await?);
    sqlx::query("UPDATE reviewables SET node_id = $2 WHERE target_type = 'post' AND target_id IN (SELECT id FROM posts WHERE item_id = $1)")
        .bind(into)
        .bind(into_node)
        .execute(&mut *tx)
        .await?;
    action_entry(&mut tx, into, actor, "merged").await?;
    recount_item(&mut tx, from).await?;
    recount_item(&mut tx, into).await?;
    recount_node(&mut tx, from_node).await?;
    recount_node(&mut tx, into_node).await?;
    recount_authors_of(&mut tx, into).await?;
    audit(&mut tx, actor, "topic.merge", format!("item:{from}"), serde_json::json!({ "into": into })).await?;
    tx.commit().await
}

/// What hangs on a topic besides its posts goes with them when it is merged into another:
/// polls (options, votes), topic votes, follows, the event, read markers (the readers keep
/// having read the topic). Rows the target already has win; the source's duplicates are dropped.
async fn move_topic_data(
    tx: &mut Transaction<'_, Postgres>,
    from: i64,
    into: i64,
    old_numbers: &[i32],
    new_numbers: &[i32],
) -> Result<(), Error> {
    // Polls: the reply a poll sits in has a new number.
    sqlx::query(
        "UPDATE polls p SET item_id = $2,
           position = p.position + (SELECT count(*) FROM polls WHERE item_id = $2)::int,
           post_number = coalesce((SELECT m.n FROM unnest($3::int[], $4::int[]) AS m(o, n) WHERE m.o = p.post_number), p.post_number)
         WHERE p.item_id = $1",
    )
    .bind(from)
    .bind(into)
    .bind(old_numbers)
    .bind(new_numbers)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE poll_options SET item_id = $2 WHERE item_id = $1")
        .bind(from)
        .bind(into)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE poll_votes SET item_id = $2 WHERE item_id = $1")
        .bind(from)
        .bind(into)
        .execute(&mut **tx)
        .await?;
    for item in [from, into] {
        sqlx::query(
            "UPDATE items SET poll_question = (SELECT question FROM polls WHERE item_id = $1 ORDER BY position, id LIMIT 1)
             WHERE id = $1",
        )
        .bind(item)
        .execute(&mut **tx)
        .await?;
    }
    // Topic votes.
    sqlx::query(
        "INSERT INTO topic_votes (item_id, user_id, created_at)
         SELECT $2, user_id, created_at FROM topic_votes WHERE item_id = $1 ON CONFLICT DO NOTHING",
    )
    .bind(from)
    .bind(into)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM topic_votes WHERE item_id = $1").bind(from).execute(&mut **tx).await?;
    sqlx::query(
        "UPDATE items SET vote_count = (SELECT count(*) FROM topic_votes WHERE item_id = items.id) WHERE id = ANY($1)",
    )
    .bind([from, into])
    .execute(&mut **tx)
    .await?;
    // Followers.
    sqlx::query(
        "INSERT INTO follows (user_id, target_type, target_id, level, email_freq)
         SELECT user_id, 'item', $2, level, email_freq FROM follows WHERE target_type = 'item' AND target_id = $1
         ON CONFLICT DO NOTHING",
    )
    .bind(from)
    .bind(into)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM follows WHERE target_type = 'item' AND target_id = $1")
        .bind(from)
        .execute(&mut **tx)
        .await?;
    // The event, and its answers with it, when the target has none of its own.
    let moved_event = sqlx::query(
        "UPDATE topic_events SET item_id = $2 WHERE item_id = $1 AND NOT EXISTS (SELECT 1 FROM topic_events WHERE item_id = $2)",
    )
    .bind(from)
    .bind(into)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if moved_event > 0 {
        sqlx::query(
            "UPDATE event_rsvps r SET item_id = $2 WHERE r.item_id = $1
               AND NOT EXISTS (SELECT 1 FROM event_rsvps x WHERE x.item_id = $2 AND x.user_id = r.user_id)",
        )
        .bind(from)
        .bind(into)
        .execute(&mut **tx)
        .await?;
    }
    // Read markers: what a member had read in the source counts as read in the target (keeping the
    // greatest), so merging does not lower anyone's "topics read".
    sqlx::query("SELECT set_config('bf.bulk_read', 'on', true)").execute(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO read_markers (user_id, item_id, last_read_post_number, bulk)
         SELECT r.user_id, $2,
                coalesce((SELECT max(m.n) FROM unnest($3::int[], $4::int[]) AS m(o, n) WHERE m.o <= r.last_read_post_number), 0),
                r.bulk
         FROM read_markers r WHERE r.item_id = $1
         ON CONFLICT (user_id, item_id) DO UPDATE SET
           last_read_post_number = greatest(read_markers.last_read_post_number, EXCLUDED.last_read_post_number),
           bulk = read_markers.bulk AND EXCLUDED.bulk",
    )
    .bind(from)
    .bind(into)
    .bind(old_numbers)
    .bind(new_numbers)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM read_markers WHERE item_id = $1").bind(from).execute(&mut **tx).await?;
    Ok(())
}

/// Moves the given replies (not the first post) into a new topic in the same forum.
/// Returns the new topic id.
pub async fn split_posts(
    pool: &PgPool,
    item_id: i64,
    post_ids: &[i64],
    title: &str,
    slug: &str,
    actor: i64,
) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let node: i64 = sqlx::query_scalar("SELECT node_id FROM items WHERE id = $1 FOR UPDATE")
        .bind(item_id)
        .fetch_one(&mut *tx)
        .await?;
    let first: (i64, Option<i64>, DateTime<Utc>) = sqlx::query_as(
        "SELECT id, author_id, created_at FROM posts WHERE item_id = $1 AND id = ANY($2) AND post_number > 1 AND action IS NULL
         ORDER BY created_at, post_number LIMIT 1",
    )
    .bind(item_id)
    .bind(post_ids)
    .fetch_one(&mut *tx)
    .await?;
    let new_id: i64 = sqlx::query_scalar(
        "INSERT INTO items (node_id, author_id, title, slug, last_poster_id, created_at)
         VALUES ($1, $2, $3, $4, $2, $5) RETURNING id",
    )
    .bind(node)
    .bind(first.1)
    .bind(title)
    .bind(slug)
    .bind(first.2)
    .fetch_one(&mut *tx)
    .await?;
    // The new topic is about the same things as the one it came from.
    sqlx::query("INSERT INTO item_tags (item_id, tag_id) SELECT $2, tag_id FROM item_tags WHERE item_id = $1")
        .bind(item_id)
        .bind(new_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "WITH moved AS (SELECT id, row_number() OVER (ORDER BY created_at, post_number) AS rn
                        FROM posts WHERE item_id = $1 AND id = ANY($2) AND post_number > 1 AND action IS NULL)
         UPDATE posts p SET item_id = $3, post_number = moved.rn FROM moved WHERE p.id = moved.id",
    )
    .bind(item_id)
    .bind(post_ids)
    .bind(new_id)
    .execute(&mut *tx)
    .await?;
    // The new first post carries the topic's state.
    sqlx::query(
        "UPDATE items SET state = (SELECT state FROM posts WHERE item_id = $1 AND post_number = 1) WHERE id = $1",
    )
    .bind(new_id)
    .execute(&mut *tx)
    .await?;
    action_entry(&mut tx, item_id, actor, "split").await?;
    recount_item(&mut tx, new_id).await?;
    recount_item(&mut tx, item_id).await?;
    recount_node(&mut tx, node).await?;
    // The new topic may differ in state from the old one; polls, votes and followers stay on the source.
    recount_authors_of(&mut tx, new_id).await?;
    recount_authors_of(&mut tx, item_id).await?;
    audit(
        &mut tx,
        actor,
        "topic.split",
        format!("item:{item_id}"),
        serde_json::json!({ "new": new_id, "posts": post_ids }),
    )
    .await?;
    tx.commit().await?;
    Ok(new_id)
}

pub async fn merged_into(pool: &PgPool, item_id: i64) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT (meta->>'merged_into')::bigint FROM items WHERE id = $1 AND state = 'deleted'")
        .bind(item_id)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}

/// Restores a soft-deleted post (and its topic, for a first post). A reply of a deleted topic is
/// not restored on its own: the call fails with `RowNotFound`, as for a post that is not deleted.
/// Strict: a post removed with an erased account is never restored, and one removed as spam or
/// rejected in review only by [`restore_post_override`].
pub async fn restore_post(pool: &PgPool, post_id: i64, actor: i64) -> Result<(), Error> {
    restore_post_override(pool, post_id, actor, false).await
}

/// Like [`restore_post`]; with `allow_reason` a moderator's deliberate decision may also restore a
/// post deleted as `spam` or `rejected`. `account_erased` is refused whatever the flag says.
pub async fn restore_post_override(pool: &PgPool, post_id: i64, actor: i64, allow_reason: bool) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    let (item_id, number, author): (i64, i32, Option<i64>) = sqlx::query_as(
        "UPDATE posts p SET state = 'visible', deleted_at = NULL, deleted_by = NULL, deleted_reason = NULL
         FROM items i WHERE p.id = $1 AND p.state = 'deleted' AND i.id = p.item_id
           AND (p.post_number = 1 OR i.state <> 'deleted')
           AND (p.deleted_reason IS NULL OR (p.deleted_reason IN ('spam', 'rejected') AND $2))
         RETURNING p.item_id, p.post_number, p.author_id",
    )
    .bind(post_id)
    .bind(allow_reason)
    .fetch_one(&mut *tx)
    .await?;
    if number == 1 {
        sqlx::query("UPDATE items SET state = 'visible', deleted_at = NULL, deleted_by = NULL WHERE id = $1")
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
    }
    recount_item(&mut tx, item_id).await?;
    let node = node_of(&mut tx, item_id).await?;
    recount_node(&mut tx, node).await?;
    if number == 1 {
        // The topic is visible again, so every author's posts in it count again.
        recount_authors_of(&mut tx, item_id).await?;
    } else if let Some(a) = author {
        sqlx::query(
            "UPDATE users SET post_count = (SELECT count(*) FROM posts p JOIN items i ON i.id = p.item_id JOIN nodes n ON n.id = i.node_id
                WHERE p.author_id = $1 AND p.state = 'visible' AND p.action IS NULL AND i.state = 'visible' AND n.counts_posts)
             WHERE id = $1",
        )
        .bind(a)
        .execute(&mut *tx)
        .await?;
    }
    audit(&mut tx, actor, "post.restore", format!("post:{post_id}"), serde_json::json!({})).await?;
    tx.commit().await
}

#[derive(Debug, Clone, FromRow)]
pub struct DeletedRow {
    pub post_id: i64,
    pub post_number: i32,
    pub item_id: i64,
    pub title: String,
    pub slug: String,
    pub author: Option<String>,
    pub deleted_by: Option<String>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub excerpt: String,
}

/// Deletion log for the moderator's forums, newest first.
pub async fn deleted_posts(pool: &PgPool, nodes: &[i64], limit: i64) -> Result<Vec<DeletedRow>, Error> {
    sqlx::query_as(
        "SELECT p.id AS post_id, p.post_number, i.id AS item_id, i.title, i.slug, a.username AS author,
                d.username AS deleted_by, p.deleted_at, left(p.raw, 301) AS excerpt
         FROM posts p JOIN items i ON i.id = p.item_id
         LEFT JOIN users a ON a.id = p.author_id LEFT JOIN users d ON d.id = p.deleted_by
         WHERE p.state = 'deleted' AND p.action IS NULL AND i.node_id = ANY($1)
           AND NOT (i.meta ? 'merged_into')
         ORDER BY p.deleted_at DESC NULLS LAST LIMIT $2",
    )
    .bind(nodes)
    .bind(limit)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, FromRow)]
pub struct RevisionRow {
    pub n: i32,
    pub hidden: bool,
    pub raw: String,
    pub editor: Option<String>,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Earlier versions of a post, newest first. Revision n holds the text before edit n.
pub async fn revisions(pool: &PgPool, post_id: i64) -> Result<Vec<RevisionRow>, Error> {
    sqlx::query_as(
        "SELECT r.n, r.hidden, r.raw, u.username AS editor, r.reason, r.created_at
         FROM post_revisions r LEFT JOIN users u ON u.id = r.editor_id WHERE r.post_id = $1 ORDER BY r.n DESC",
    )
    .bind(post_id)
    .fetch_all(pool)
    .await
}

/// Poster IP for staff (post actions).
pub async fn post_ip(pool: &PgPool, post_id: i64) -> Result<Option<String>, Error> {
    sqlx::query_scalar::<_, Option<sqlx::types::ipnetwork::IpNetwork>>("SELECT ip FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_optional(pool)
        .await
        .map(|r| r.flatten().map(|ip| ip.ip().to_string()))
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct Note {
    pub id: i64,
    pub author: Option<String>,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

pub async fn notes(pool: &PgPool, user_id: i64) -> Result<Vec<Note>, Error> {
    sqlx::query_as(
        "SELECT n.id, a.username AS author, n.body, n.created_at FROM user_notes n LEFT JOIN users a ON a.id = n.author_id
         WHERE n.user_id = $1 ORDER BY n.created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// A staff note on a post.
#[derive(Debug, Clone, FromRow)]
pub struct PostNote {
    pub id: i64,
    pub post_id: i64,
    pub author: Option<String>,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// The notes on these posts, oldest first.
pub async fn post_notes(pool: &PgPool, post_ids: &[i64]) -> Result<Vec<PostNote>, Error> {
    sqlx::query_as(
        "SELECT n.id, n.post_id, a.username AS author, n.body, n.created_at FROM post_notes n
         LEFT JOIN users a ON a.id = n.author_id WHERE n.post_id = ANY($1) ORDER BY n.created_at, n.id",
    )
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

pub async fn add_post_note(pool: &PgPool, post: i64, author: i64, body: &str) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let id: i64 =
        sqlx::query_scalar("INSERT INTO post_notes (post_id, author_id, body) VALUES ($1, $2, $3) RETURNING id")
            .bind(post)
            .bind(author)
            .bind(body)
            .fetch_one(&mut *tx)
            .await?;
    record(
        &mut *tx,
        Entry::new("post.note.add").by(author).on(format!("post:{post}")).details(serde_json::json!({ "note": id })),
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// Removes a note of this post; false when there is no such note on it.
pub async fn delete_post_note(pool: &PgPool, post: i64, note: i64, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let gone = sqlx::query("DELETE FROM post_notes WHERE id = $1 AND post_id = $2")
        .bind(note)
        .bind(post)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    if gone {
        record(
            &mut *tx,
            Entry::new("post.note.delete")
                .by(actor)
                .on(format!("post:{post}"))
                .details(serde_json::json!({ "note": note })),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(gone)
}

pub async fn add_note(pool: &PgPool, user_id: i64, author: i64, body: &str) -> Result<(), Error> {
    sqlx::query("INSERT INTO user_notes (user_id, author_id, body) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(author)
        .bind(body)
        .execute(pool)
        .await
        .map(|_| ())
}

/// "Delete spammer": every visible post of the user is deleted (topics they
/// started go with their first post), the account is suspended, and it is all logged.
pub async fn delete_spammer(pool: &PgPool, user_id: i64, actor: Option<i64>) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let items: Vec<i64> = sqlx::query_scalar(
        "WITH d AS (UPDATE posts SET state = 'deleted', deleted_at = now(), deleted_by = $2, deleted_reason = 'spam'
                    WHERE author_id = $1 AND state <> 'deleted' RETURNING item_id, post_number)
         SELECT DISTINCT item_id FROM d",
    )
    .bind(user_id)
    .bind(actor)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE reviewables r SET status = 'rejected', resolved_at = now(), resolved_by = $2
         FROM posts p WHERE r.target_type = 'post' AND r.target_id = p.id AND r.status = 'pending' AND p.author_id = $1",
    )
    .bind(user_id)
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE items SET state = 'deleted', deleted_at = now(), deleted_by = $2
         WHERE author_id = $1 AND state <> 'deleted'",
    )
    .bind(user_id)
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    for item in &items {
        recount_item(&mut tx, *item).await?;
        // A topic that went with its first post takes everyone's posts in it out of their totals.
        recount_authors_of(&mut tx, *item).await?;
    }
    let nodes: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT node_id FROM items WHERE id = ANY($1)")
        .bind(&items)
        .fetch_all(&mut *tx)
        .await?;
    for n in nodes {
        recount_node(&mut tx, n).await?;
    }
    sqlx::query("UPDATE users SET suspended_until = 'infinity', post_count = 0 WHERE id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(user_id).execute(&mut *tx).await?;
    audit(
        &mut tx,
        actor,
        "user.delete_spammer",
        format!("user:{user_id}"),
        serde_json::json!({ "topics": items.len() }),
    )
    .await?;
    tx.commit().await?;
    Ok(items.len() as i64)
}

/// Marks a post that was just deleted as removed for `reason` (`rejected`); no effect on one that is not deleted.
pub async fn set_deleted_reason(pool: &PgPool, post_id: i64, reason: &str) -> Result<(), Error> {
    sqlx::query("UPDATE posts SET deleted_reason = $2 WHERE id = $1 AND state = 'deleted'")
        .bind(post_id)
        .bind(reason)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Staff hide or show one revision (it keeps its row).
pub async fn set_revision_hidden(pool: &PgPool, post_id: i64, n: i32, hidden: bool, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let done = sqlx::query("UPDATE post_revisions SET hidden = $3 WHERE post_id = $1 AND n = $2")
        .bind(post_id)
        .bind(n)
        .bind(hidden)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    audit(
        &mut tx,
        actor,
        if hidden { "revision.hide" } else { "revision.show" },
        format!("post:{post_id}"),
        serde_json::json!({ "n": n }),
    )
    .await?;
    tx.commit().await?;
    Ok(done)
}

pub async fn revision_raw(pool: &PgPool, post_id: i64, n: i32) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT raw FROM post_revisions WHERE post_id = $1 AND n = $2")
        .bind(post_id)
        .bind(n)
        .fetch_optional(pool)
        .await
}

pub async fn set_edit_locked(pool: &PgPool, post_id: i64, locked: bool, actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE posts SET edit_locked = $2 WHERE id = $1").bind(post_id).bind(locked).execute(&mut *tx).await?;
    audit(
        &mut tx,
        actor,
        if locked { "post.edit_lock" } else { "post.edit_unlock" },
        format!("post:{post_id}"),
        serde_json::json!({}),
    )
    .await?;
    tx.commit().await
}
