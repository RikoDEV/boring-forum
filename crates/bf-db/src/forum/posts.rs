//! Posts: creation, edits, states, counts and the cooked HTML cache.

use chrono::{DateTime, Utc};
use crate::admin::audit::{Entry, record};
use sqlx::{FromRow, PgPool, Postgres, Transaction};

type Error = sqlx::Error;

#[derive(Debug, Clone, FromRow)]
pub struct PostRow {
    pub id: i64,
    pub post_number: i32,
    pub author_id: Option<i64>,
    pub author: Option<String>,
    pub cooked: String,
    pub action: Option<String>,
    pub state: String,
    pub edit_count: i32,
    pub like_count: i32,
    pub hidden_reason: Option<String>,
    /// The post this one answers: (number, author).
    pub reply_to_number: Option<i32>,
    pub reply_to_author: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Poster IP as text; only shown to moderators.
    pub liked: bool,
    pub bookmarked: bool,
    pub ignored: bool,
    pub reply_total: i64,
    pub reply_first: Option<i32>,
    /// Counts by reaction, such as "♥ 2 · 😄 1"; empty without reactions.
    pub wiki: bool,
    pub author_signature: Option<String>,
    pub reaction_summary: String,
    pub my_reaction: Option<i64>,
    pub author_reputation: Option<i64>,
    pub author_posts: Option<i32>,
    pub author_trust_level: Option<i16>,
    pub author_group: Option<String>,
    pub author_special: Option<bool>,
    pub author_group_colour: Option<String>,
    pub author_group_icon: Option<String>,
    pub author_group_image: Option<String>,
    /// Answer votes (Q&A forums), and this viewer's: 1, -1 or None.
    pub vote_score: i32,
    pub my_vote: Option<i16>,
}

/// Columns and joins of a post row as a topic page shows it. `$5` is the viewer (NULL for guests).
macro_rules! post_select {
    () => {
        "SELECT p.id, p.post_number, p.author_id, u.username AS author, p.cooked, p.action, p.state, p.edit_count, p.like_count, p.hidden_reason, p.wiki,
                rp.post_number AS reply_to_number, CASE WHEN rp.state = 'visible' THEN ru.username END AS reply_to_author, p.created_at,
                $5::bigint IS NOT NULL AND EXISTS (SELECT 1 FROM post_likes l WHERE l.post_id = p.id AND l.user_id = $5) AS liked,
                $5::bigint IS NOT NULL AND EXISTS (SELECT 1 FROM bookmarks b WHERE b.post_id = p.id AND b.user_id = $5) AS bookmarked,
                $5::bigint IS NOT NULL AND EXISTS (SELECT 1 FROM ignored_users g WHERE g.user_id = $5 AND g.ignored_user_id = p.author_id) AS ignored,
                (SELECT count(*) FROM posts r WHERE r.reply_to_post_id = p.id AND r.state = 'visible') AS reply_total,
                (SELECT min(r.post_number) FROM posts r WHERE r.reply_to_post_id = p.id AND r.state = 'visible') AS reply_first,
                coalesce((SELECT string_agg(x.icon || ' ' || x.n, ' · ' ORDER BY x.position)
                          FROM (SELECT re.icon, re.position, count(*) AS n FROM post_likes pl JOIN reactions re ON re.id = pl.reaction_id
                                WHERE pl.post_id = p.id GROUP BY re.icon, re.position) x), '') AS reaction_summary,
                (SELECT pl.reaction_id FROM post_likes pl WHERE pl.post_id = p.id AND pl.user_id = $5) AS my_reaction,
                u.reputation AS author_reputation, u.signature_html AS author_signature,
                u.post_count AS author_posts, u.trust_level AS author_trust_level,
                g.name AS author_group, g.id <> 2 AS author_special,
                g.colour AS author_group_colour, g.icon AS author_group_icon, g.image AS author_group_image,
                p.vote_score, (SELECT pv.value FROM post_votes pv WHERE pv.post_id = p.id AND pv.user_id = $5) AS my_vote
         FROM posts p LEFT JOIN users u ON u.id = p.author_id
         LEFT JOIN groups g ON g.id = u.primary_group_id
         LEFT JOIN posts rp ON rp.id = p.reply_to_post_id LEFT JOIN users ru ON ru.id = rp.author_id"
    };
}

/// One topic page. Keyset on `post_number` rather than OFFSET.
pub async fn topic_posts(
    pool: &PgPool,
    item_id: i64,
    from_number: i32,
    limit: i64,
    include_hidden: bool,
    viewer: Option<i64>,
) -> Result<Vec<PostRow>, Error> {
    // Only the constant POST_SELECT is interpolated; every value is bound.
    sqlx::query_as(concat!(post_select!(), "
         WHERE p.item_id = $1 AND p.post_number >= $2 AND p.post_number < $2 + $3 AND (p.state = 'visible' OR $4)
         ORDER BY p.post_number LIMIT $3"))
    .bind(item_id)
    .bind(from_number)
    .bind(limit)
    .bind(include_hidden)
    .bind(viewer)
    .fetch_all(pool)
    .await
}

/// A Q&A topic on one page: the question, the accepted answer, then answers by votes, oldest
/// first among equals. Small action entries are left out; they would sit between answers.
pub async fn topic_posts_ranked(
    pool: &PgPool,
    item_id: i64,
    limit: i64,
    include_hidden: bool,
    viewer: Option<i64>,
) -> Result<Vec<PostRow>, Error> {
    // Only the constant POST_SELECT is interpolated; every value is bound.
    sqlx::query_as(concat!(post_select!(), "
         WHERE p.item_id = $1 AND $2 = 0 AND p.action IS NULL AND (p.state = 'visible' OR $4)
         ORDER BY CASE WHEN p.post_number = 1 THEN 0
                       WHEN p.post_number = (SELECT solved_post_number FROM items WHERE id = $1) THEN 1 ELSE 2 END,
                  p.vote_score DESC, p.post_number
         LIMIT $3"))
    .bind(item_id)
    .bind(0i32)
    .bind(limit)
    .bind(include_hidden)
    .bind(viewer)
    .fetch_all(pool)
    .await
}

/// Sets this member's vote on an answer: 1, -1, or 0 to take it back. Returns the new score.
pub async fn vote_post(pool: &PgPool, post_id: i64, user: i64, value: i16) -> Result<i32, Error> {
    let mut tx = pool.begin().await?;
    if value == 0 {
        sqlx::query("DELETE FROM post_votes WHERE post_id = $1 AND user_id = $2")
            .bind(post_id)
            .bind(user)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO post_votes (post_id, user_id, value) VALUES ($1, $2, $3)
             ON CONFLICT (post_id, user_id) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(post_id)
        .bind(user)
        .bind(value)
        .execute(&mut *tx)
        .await?;
    }
    let score: i32 = sqlx::query_scalar(
        "UPDATE posts SET vote_score = (SELECT coalesce(sum(value), 0)::int FROM post_votes WHERE post_id = $1)
         WHERE id = $1 RETURNING vote_score",
    )
    .bind(post_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(score)
}

#[derive(Clone, Copy)]
pub struct NewPost<'a> {
    pub author_id: i64,
    pub raw: &'a str,
    pub cooked: &'a str,
    pub cooked_version: i32,
    pub ip: Option<std::net::IpAddr>,
    /// Held for moderator approval (watched words, approval rules).
    pub pending: bool,
}

/// Points a new reply at the post it answers (same topic only).
pub async fn set_reply_to(pool: &PgPool, item_id: i64, number: i32, reply_to_number: i32) -> Result<(), Error> {
    sqlx::query(
        "UPDATE posts SET reply_to_post_id = (SELECT id FROM posts WHERE item_id = $1 AND post_number = $3)
         WHERE item_id = $1 AND post_number = $2 AND $3 < $2",
    )
    .bind(item_id)
    .bind(number)
    .bind(reply_to_number)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Up to five recent distinct posters per topic, in one query for a whole list.
pub async fn posters(pool: &PgPool, items: &[i64]) -> Result<Vec<(i64, Vec<String>)>, Error> {
    sqlx::query_as(
        "SELECT item_id, (array_agg(username ORDER BY last DESC))[1:5] FROM (
           SELECT p.item_id, u.username, max(p.post_number) AS last FROM posts p JOIN users u ON u.id = p.author_id
           WHERE p.item_id = ANY($1) AND p.state = 'visible' AND p.action IS NULL GROUP BY p.item_id, u.username) t
         GROUP BY item_id",
    )
    .bind(items)
    .fetch_all(pool)
    .await
}

/// Topic and number of a post, for short links and post-number jumps.
pub async fn locate_post(pool: &PgPool, post_id: i64) -> Result<Option<(i64, i32)>, Error> {
    sqlx::query_as("SELECT item_id, post_number FROM posts WHERE id = $1").bind(post_id).fetch_optional(pool).await
}

pub(crate) async fn insert_post(
    tx: &mut Transaction<'_, Postgres>,
    item_id: i64,
    p: &NewPost<'_>,
) -> Result<(i64, i32), Error> {
    // Lock the topic row so post numbers are assigned without gaps or duplicates.
    // Two statements: one that waited for the lock still reads its old snapshot, so the number
    // must come from a statement that starts after the lock is held.
    sqlx::query("SELECT id FROM items WHERE id = $1 FOR UPDATE").bind(item_id).fetch_one(&mut **tx).await?;
    // Never below the highest number the topic has had: readers' markers only grow, so a number
    // freed by a split or purge must not be given to a new reply.
    let next: i32 = sqlx::query_scalar(
        "SELECT greatest(coalesce(max(post_number), 0), (SELECT highest_post_number FROM items WHERE id = $1)) + 1
         FROM posts WHERE item_id = $1",
    )
        .bind(item_id)
        .fetch_one(&mut **tx)
        .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO posts (item_id, author_id, post_number, raw, cooked, cooked_version, ip, state)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(item_id)
    .bind(p.author_id)
    .bind(next)
    .bind(p.raw)
    .bind(p.cooked)
    .bind(p.cooked_version)
    .bind(p.ip.map(sqlx::types::ipnetwork::IpNetwork::from))
    .bind(if p.pending { "pending" } else { "visible" })
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("UPDATE items SET highest_post_number = greatest(highest_post_number, $2) WHERE id = $1")
        .bind(item_id)
        .bind(next)
        .execute(&mut **tx)
        .await?;
    if p.pending {
        sqlx::query(
            "INSERT INTO reviewables (kind, target_type, target_id, node_id)
             SELECT 'pending_post', 'post', $1, node_id FROM items WHERE id = $2",
        )
        .bind(id)
        .bind(item_id)
        .execute(&mut **tx)
        .await?;
    } else {
        // Some forums (off-topic, testing) do not count toward the author's total.
        sqlx::query(
            "UPDATE users SET post_count = post_count + 1 WHERE id = $1 AND
               (SELECT n.counts_posts FROM items i JOIN nodes n ON n.id = i.node_id WHERE i.id = $2)",
        )
        .bind(p.author_id)
        .bind(item_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok((id, next))
}

/// Creates a topic and its first post, updating denormalized counters in the same transaction.
pub async fn create_topic(
    pool: &PgPool,
    node_id: i64,
    title: &str,
    slug: &str,
    post: &NewPost<'_>,
) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let item_id: i64 = sqlx::query_scalar(
        "INSERT INTO items (node_id, author_id, title, slug, last_poster_id, state)
         VALUES ($1, $2, $3, $4, $2, $5) RETURNING id",
    )
    .bind(node_id)
    .bind(post.author_id)
    .bind(title)
    .bind(slug)
    .bind(if post.pending { "pending" } else { "visible" })
    .fetch_one(&mut *tx)
    .await?;
    let (post_id, _) = insert_post(&mut tx, item_id, post).await?;
    sqlx::query("UPDATE items SET last_post_id = $2 WHERE id = $1")
        .bind(item_id)
        .bind(post_id)
        .execute(&mut *tx)
        .await?;
    if !post.pending {
        sqlx::query(
            "UPDATE nodes SET topic_count = topic_count + 1, post_count = post_count + 1,
             last_item_id = $2, last_activity_at = now() WHERE id = $1",
        )
        .bind(node_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(item_id)
}

pub async fn create_reply(pool: &PgPool, item_id: i64, node_id: i64, post: &NewPost<'_>) -> Result<i32, Error> {
    reply_in_tx(pool, item_id, node_id, post, None).await
}

/// A reply written under the shared anonymous account from the first statement. `post.author_id`
/// is the real member: it is kept in `real_author_id` (for limits) and the audit log, never shown.
/// Returns the post number and the id of the account it appears under.
pub async fn create_anonymous_reply(
    pool: &PgPool,
    item_id: i64,
    node_id: i64,
    post: &NewPost<'_>,
) -> Result<(i32, i64), Error> {
    let anon = crate::forum::anonymous::anonymous_account(pool).await?;
    let shown = NewPost { author_id: anon, ip: None, ..*post };
    let number = reply_in_tx(pool, item_id, node_id, &shown, Some(post.author_id)).await?;
    Ok((number, anon))
}

async fn reply_in_tx(
    pool: &PgPool,
    item_id: i64,
    node_id: i64,
    post: &NewPost<'_>,
    real: Option<i64>,
) -> Result<i32, Error> {
    let mut tx = pool.begin().await?;
    let (post_id, number) = insert_post(&mut tx, item_id, post).await?;
    if let Some(real) = real {
        sqlx::query("UPDATE posts SET real_author_id = $2 WHERE id = $1")
            .bind(post_id)
            .bind(real)
            .execute(&mut *tx)
            .await?;
        record(&mut *tx, Entry::new("post.anonymous").by(real).on(item_id.to_string()).details(serde_json::json!({ "number": number }))).await?;
    }
    if post.pending {
        tx.commit().await?;
        return Ok(number);
    }
    sqlx::query(
        "UPDATE items SET reply_count = reply_count + 1, last_post_id = $2, last_posted_at = now(),
         last_poster_id = $3, updated_at = now() WHERE id = $1",
    )
    .bind(item_id)
    .bind(post_id)
    .bind(post.author_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE nodes SET post_count = post_count + 1, last_item_id = $2, last_activity_at = now() WHERE id = $1",
    )
    .bind(node_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(number)
}

/// Edits a post, keeping the previous text as a revision.
pub async fn edit_post(
    pool: &PgPool,
    post_id: i64,
    editor_id: i64,
    raw: &str,
    cooked: &str,
    cooked_version: i32,
    reason: Option<&str>,
) -> Result<(), Error> {
    edit_post_if(pool, post_id, editor_id, raw, cooked, cooked_version, reason, None).await.map(|_| ())
}

/// `edit_post` that only applies when the post still has `expected` edits (read under a row lock,
/// so two saves from the same base cannot both pass). `Ok(false)` means someone else edited first.
#[expect(clippy::too_many_arguments, reason = "mirrors edit_post plus the expected edit count")]
pub async fn edit_post_if(
    pool: &PgPool,
    post_id: i64,
    editor_id: i64,
    raw: &str,
    cooked: &str,
    cooked_version: i32,
    reason: Option<&str>,
    expected: Option<i32>,
) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let current: i32 = sqlx::query_scalar("SELECT edit_count FROM posts WHERE id = $1 FOR UPDATE")
        .bind(post_id)
        .fetch_one(&mut *tx)
        .await?;
    if expected.is_some_and(|e| e != current) {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO post_revisions (post_id, n, raw, editor_id, reason)
         SELECT id, edit_count + 1, raw, $2, $3 FROM posts WHERE id = $1",
    )
    .bind(post_id)
    .bind(editor_id)
    .bind(reason)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE posts SET raw = $2, cooked = $3, cooked_version = $4, edit_count = edit_count + 1,
         last_editor_id = $5, updated_at = now() WHERE id = $1",
    )
    .bind(post_id)
    .bind(raw)
    .bind(cooked)
    .bind(cooked_version)
    .bind(editor_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

#[derive(Debug, Clone, FromRow)]
pub struct PostRef {
    pub id: i64,
    pub item_id: i64,
    pub author_id: Option<i64>,
    pub raw: String,
    pub post_number: i32,
    pub node_id: i64,
    pub edit_count: i32,
    pub created_at: DateTime<Utc>,
    pub edit_locked: bool,
    pub state: String,
}

pub async fn post_ref(pool: &PgPool, post_id: i64) -> Result<Option<PostRef>, Error> {
    sqlx::query_as(
        "SELECT p.id, p.item_id, p.author_id, p.raw, p.post_number, i.node_id, p.edit_count, p.created_at, p.edit_locked, p.state::text AS state
         FROM posts p JOIN items i ON i.id = p.item_id WHERE p.id = $1 AND p.state <> 'deleted'",
    )
    .bind(post_id)
    .fetch_optional(pool)
    .await
}

/// Soft delete: the row stays for the deletion log and restore.
/// Hides a post from members (moderators still see it), with a reason; or shows it again.
pub async fn set_hidden(pool: &PgPool, post_id: i64, hidden: bool, reason: Option<&str>, by: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    set_post_state(&mut tx, post_id, if hidden { "hidden" } else { "visible" }, Some(by)).await?;
    sqlx::query("UPDATE posts SET hidden_reason = $2 WHERE id = $1")
        .bind(post_id)
        .bind(if hidden { reason } else { None })
        .execute(&mut *tx)
        .await?;
    record(&mut *tx, Entry::new(if hidden { "post.hide" } else { "post.unhide" }).by(by).on(format!("post:{post_id}")).details(serde_json::json!({ "reason": reason }))).await?;
    tx.commit().await
}

/// An edit added something that needs approval: a visible post goes back into the review
/// queue until a moderator looks. False when the post was not visible.
pub async fn requeue_post(pool: &PgPool, post_id: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM posts WHERE id = $1 FOR UPDATE")
        .bind(post_id)
        .fetch_optional(&mut *tx)
        .await?;
    if state.as_deref() != Some("visible") {
        return Ok(false);
    }
    set_post_state(&mut tx, post_id, "pending", None).await?;
    sqlx::query(
        "INSERT INTO reviewables (kind, target_type, target_id, node_id, payload)
         SELECT 'pending_post', 'post', p.id, i.node_id, '{\"edited\": true}' FROM posts p JOIN items i ON i.id = p.item_id
         WHERE p.id = $1
         ON CONFLICT (target_type, target_id, kind) DO UPDATE SET status = 'pending', resolved_by = NULL,
           resolved_at = NULL, claimed_by = NULL, created_at = now(), payload = EXCLUDED.payload",
    )
    .bind(post_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// A held post goes public, with every counter it feeds.
pub async fn restore_to_visible(pool: &PgPool, post_id: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    set_post_state(&mut tx, post_id, "visible", None).await?;
    tx.commit().await
}

pub async fn soft_delete_post(pool: &PgPool, post_id: i64, by: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    set_post_state(&mut tx, post_id, "deleted", Some(by)).await?;
    tx.commit().await
}

/// Changes a post's state and recomputes every counter it feeds. The first post
/// carries the topic: its state becomes the topic's state.
pub(crate) async fn set_post_state(
    tx: &mut Transaction<'_, Postgres>,
    post_id: i64,
    state: &str,
    by: Option<i64>,
) -> Result<(), Error> {
    set_post_state_with_reason(tx, post_id, state, by, None).await
}

/// `set_post_state` that also records why a deletion happened (`spam`, `account_erased`,
/// `rejected`; `None` is an ordinary one). The reason is cleared by any other state.
pub(crate) async fn set_post_state_with_reason(
    tx: &mut Transaction<'_, Postgres>,
    post_id: i64,
    state: &str,
    by: Option<i64>,
    reason: Option<&str>,
) -> Result<(), Error> {
    // A deleted post leaves nothing to review; "keep" on a stale item would bring it back.
    if state == "deleted" {
        sqlx::query(
            "UPDATE reviewables SET status = 'rejected', resolved_at = now(), resolved_by = $2
             WHERE target_type = 'post' AND target_id = $1 AND status = 'pending'",
        )
        .bind(post_id)
        .bind(by)
        .execute(&mut **tx)
        .await?;
    }
    let (item_id, number, author): (i64, i32, Option<i64>) = sqlx::query_as(
        "UPDATE posts SET state = $2,
           deleted_at = CASE WHEN $2 = 'deleted' THEN now() END, deleted_by = CASE WHEN $2 = 'deleted' THEN $3 END,
           deleted_reason = CASE WHEN $2 = 'deleted' THEN $4 END
         WHERE id = $1 RETURNING item_id, post_number, author_id",
    )
    .bind(post_id)
    .bind(state)
    .bind(by)
    .bind(reason)
    .fetch_one(&mut **tx)
    .await?;
    if number == 1 {
        sqlx::query(
            "UPDATE items SET state = $2, deleted_at = CASE WHEN $2 = 'deleted' THEN now() END,
             deleted_by = CASE WHEN $2 = 'deleted' THEN $3 END WHERE id = $1",
        )
        .bind(item_id)
        .bind(state)
        .bind(by)
        .execute(&mut **tx)
        .await?;
    }
    recount(tx, item_id, author).await?;
    if number == 1 {
        // The topic's visibility changed, which changes every author's count in it.
        crate::moderation::topics::recount_authors_of(tx, item_id).await?;
    }
    Ok(())
}

/// Recomputes topic, forum and author counters from the rows themselves. Used on the
/// rarer moderation paths; posting updates counters incrementally.
async fn recount(tx: &mut Transaction<'_, Postgres>, item_id: i64, author_id: Option<i64>) -> Result<(), Error> {
    sqlx::query(
        "UPDATE items i SET
           reply_count = greatest(coalesce(s.n, 0) - 1, 0),
           last_post_id = coalesce(s.last_id, i.last_post_id),
           last_posted_at = coalesce(s.last_at, i.created_at),
           last_poster_id = coalesce(s.last_author, i.author_id)
         FROM (SELECT count(*) AS n,
                 (array_agg(id ORDER BY post_number DESC))[1] AS last_id,
                 max(created_at) AS last_at,
                 (array_agg(author_id ORDER BY post_number DESC))[1] AS last_author
               FROM posts WHERE item_id = $1 AND state = 'visible' AND action IS NULL) s
         WHERE i.id = $1",
    )
    .bind(item_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE nodes n SET topic_count = coalesce(s.topics, 0), post_count = coalesce(s.posts, 0),
           last_item_id = s.last_item, last_activity_at = s.last_at
         FROM (SELECT count(*) AS topics, sum(reply_count + 1) AS posts,
                 (array_agg(id ORDER BY last_posted_at DESC))[1] AS last_item, max(last_posted_at) AS last_at
               FROM items WHERE node_id = (SELECT node_id FROM items WHERE id = $1) AND state = 'visible') s
         WHERE n.id = (SELECT node_id FROM items WHERE id = $1)",
    )
    .bind(item_id)
    .execute(&mut **tx)
    .await?;
    if let Some(a) = author_id {
        sqlx::query(
            "UPDATE users SET post_count =
               (SELECT count(*) FROM posts p JOIN items i ON i.id = p.item_id JOIN nodes n ON n.id = i.node_id
                WHERE p.author_id = $1 AND p.state = 'visible' AND p.action IS NULL AND i.state = 'visible' AND n.counts_posts)
             WHERE id = $1",
        )
        .bind(a)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// For the live-update polling fallback: the topic's newest post number.
/// The newest post number a member can see (visible posts only).
pub async fn highest_visible_post_number(pool: &PgPool, item_id: i64) -> Result<Option<i32>, Error> {
    sqlx::query_scalar("SELECT max(post_number) FROM posts WHERE item_id = $1 AND state = 'visible'")
        .bind(item_id)
        .fetch_one(pool)
        .await
}

pub async fn highest_post_number(pool: &PgPool, item_id: i64) -> Result<Option<i32>, Error> {
    sqlx::query_scalar("SELECT highest_post_number FROM items WHERE id = $1").bind(item_id).fetch_optional(pool).await
}

/// An edit inside the grace period: replaces the text without a revision.
pub async fn replace_post_text(
    pool: &PgPool,
    post_id: i64,
    raw: &str,
    cooked: &str,
    cooked_version: i32,
) -> Result<(), Error> {
    sqlx::query("UPDATE posts SET raw = $2, cooked = $3, cooked_version = $4, updated_at = now() WHERE id = $1")
        .bind(post_id)
        .bind(raw)
        .bind(cooked)
        .bind(cooked_version)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Posts cooked by an older cooker, for the re-cook job: (id, raw).
pub async fn stale_cooked(pool: &PgPool, version: i32, limit: i64) -> Result<Vec<(i64, String)>, Error> {
    sqlx::query_as("SELECT id, raw FROM posts WHERE cooked_version < $1 AND action IS NULL ORDER BY id LIMIT $2")
        .bind(version)
        .bind(limit)
        .fetch_all(pool)
        .await
}

pub async fn save_cooked(pool: &PgPool, ids: &[i64], html: &[String], version: i32) -> Result<(), Error> {
    sqlx::query(
        "UPDATE posts SET cooked = v.html, cooked_version = $3
         FROM unnest($1::bigint[], $2::text[]) AS v(id, html) WHERE posts.id = v.id",
    )
    .bind(ids)
    .bind(html)
    .bind(version)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Adds a moderator's reason to the action entry just written for a topic.
pub async fn add_action_reason(pool: &PgPool, item_id: i64, reason: &str) -> Result<(), Error> {
    sqlx::query(
        "UPDATE posts SET action = action || ': ' || $2
         WHERE item_id = $1 AND post_number = (SELECT max(post_number) FROM posts WHERE item_id = $1) AND action IS NOT NULL",
    )
    .bind(item_id)
    .bind(reason)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Replies to each of these posts: (post id, replies, first reply's number).
pub async fn reply_counts(pool: &PgPool, posts: &[i64]) -> Result<Vec<(i64, i64, i32)>, Error> {
    sqlx::query_as(
        "SELECT reply_to_post_id, count(*), min(post_number) FROM posts
         WHERE reply_to_post_id = ANY($1) AND state = 'visible' GROUP BY reply_to_post_id",
    )
    .bind(posts)
    .fetch_all(pool)
    .await
}

/// A visible post's cooked HTML and author, for "show reply context".
pub async fn post_context(pool: &PgPool, item: i64, number: i32) -> Result<Option<(String, Option<String>)>, Error> {
    sqlx::query_as(
        "SELECT p.cooked, u.username FROM posts p LEFT JOIN users u ON u.id = p.author_id
         WHERE p.item_id = $1 AND p.post_number = $2 AND p.state = 'visible' AND p.action IS NULL",
    )
    .bind(item)
    .bind(number)
    .fetch_optional(pool)
    .await
}

/// Removes content deleted longer ago than `days` for good.
pub async fn purge_deleted(pool: &PgPool, days: i64) -> Result<u64, Error> {
    let items = sqlx::query(
        "DELETE FROM items WHERE state = 'deleted' AND deleted_at < now() - make_interval(days => $1::int)",
    )
    .bind(days)
    .execute(pool)
    .await?
    .rows_affected();
    let posts = sqlx::query(
        "DELETE FROM posts p WHERE p.state = 'deleted' AND p.deleted_at < now() - make_interval(days => $1::int) AND p.post_number > 1",
    )
    .bind(days)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(items + posts)
}

/// Which of these (topic id, post number) pairs point at deleted posts.
pub async fn deleted_refs(pool: &PgPool, items: &[i64], numbers: &[i32]) -> Result<Vec<(i64, i32)>, Error> {
    sqlx::query_as(
        "SELECT p.item_id, p.post_number FROM posts p JOIN unnest($1::bigint[], $2::int[]) AS r(item, num)
           ON p.item_id = r.item AND p.post_number = r.num WHERE p.state = 'deleted'",
    )
    .bind(items)
    .bind(numbers)
    .fetch_all(pool)
    .await
}

/// For each post: when its author last posted anywhere, and when the topic last had a post.
/// (post id, author's previous post, topic's previous post)
pub async fn post_timeline(
    pool: &PgPool,
    ids: &[i64],
) -> Result<Vec<(i64, Option<DateTime<Utc>>, Option<DateTime<Utc>>)>, Error> {
    sqlx::query_as(
        "SELECT p.id,
                (SELECT max(q.created_at) FROM posts q WHERE q.author_id = p.author_id AND q.created_at < p.created_at
                   AND q.state = 'visible' AND q.action IS NULL),
                (SELECT max(r.created_at) FROM posts r WHERE r.item_id = p.item_id AND r.post_number < p.post_number
                   AND r.state = 'visible')
         FROM posts p WHERE p.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// Seconds since the user's last post (flood wait).
/// Posts of this member that are public, in any forum, counted up to `up_to`: what "the first
/// N posts are held" needs, unlike `users.post_count`, which skips forums that do not count posts.
pub async fn approved_post_count(pool: &PgPool, user_id: i64, up_to: i64) -> Result<i64, Error> {
    // Only "fewer than N?" matters, so a prolific member costs at most N index entries.
    if up_to <= 0 {
        return Ok(0);
    }
    sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT 1 FROM posts WHERE (author_id = $1 OR real_author_id = $1) AND state = 'visible' AND action IS NULL
           LIMIT $2) x",
    )
    .bind(user_id)
    .bind(up_to)
    .fetch_one(pool)
    .await
}

pub async fn seconds_since_last_post(pool: &PgPool, user_id: i64) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - max(created_at))::bigint FROM posts WHERE (author_id = $1 OR real_author_id = $1) AND action IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

pub async fn newest_posts(pool: &PgPool, limit: i64) -> Result<Vec<(DateTime<Utc>, String)>, Error> {
    sqlx::query_as("SELECT created_at, raw FROM posts ORDER BY id DESC LIMIT $1").bind(limit).fetch_all(pool).await
}
