//! Topics: lists, flags, pins, prefixes, scheduling, copies and moved-topic redirects.

use crate::admin::audit::{Entry, record};
use crate::forum::posts::NewPost;
use crate::forum::posts::insert_post;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

use crate::moderation::tools::audit;

type Error = sqlx::Error;

#[derive(Debug, Clone, FromRow)]
pub struct TopicRow {
    pub id: i64,
    pub node_id: i64,
    pub title: String,
    pub slug: String,
    pub author: Option<String>,
    pub author_id: Option<i64>,
    pub pinned: bool,
    pub locked: bool,
    pub archived: bool,
    pub featured: bool,
    pub listed: bool,
    pub state: String,
    pub reply_count: i32,
    pub view_count: i32,
    pub last_posted_at: DateTime<Utc>,
    pub last_poster: Option<String>,
    pub created_at: DateTime<Utc>,
    pub highest_post_number: i32,
    pub prefix: Option<String>,
    pub solved_post_number: Option<i32>,
    pub poll_question: Option<String>,
    pub assigned_to: Option<i64>,
    /// The topic carries a calendar event.
    pub has_event: bool,
    /// The topic hosts a live Q&A session.
    pub has_live: bool,
    /// Votes for the topic (ideas forums), and whether the viewer gave one.
    pub vote_count: i32,
    pub voted: bool,
    /// Highest post number the viewer has read; `None` for guests.
    pub last_read: Option<i32>,
    /// `name|slug` of each tag.
    pub follow_level: Option<String>,
    pub tag_pairs: Vec<String>,
    /// Recent posters, newest first.
    pub posters: Vec<String>,
}

impl TopicRow {
    /// Unread posts for the viewer, or `None` for guests and fully read topics.
    pub fn unread(&self) -> Option<i32> {
        let n = self.highest_post_number - self.last_read?;
        (n > 0).then_some(n)
    }
}

/// `last_read` resolves, in order: the topic's read marker; "mark forum read" for
/// topics with no newer posts; and topics older than the account count as read.
pub(crate) const TOPIC_COLS: &str =
    "i.id, i.node_id, i.title, i.slug, a.username AS author, i.author_id, i.pinned, i.locked, i.archived, i.featured, i.listed, i.state,
    i.reply_count, i.view_count, i.last_posted_at, lp.username AS last_poster, i.created_at, i.highest_post_number, i.prefix, i.solved_post_number, i.poll_question, i.assigned_to,
    EXISTS (SELECT 1 FROM topic_events te WHERE te.item_id = i.id) AS has_event,
    EXISTS (SELECT 1 FROM live_sessions ls WHERE ls.item_id = i.id) AS has_live,
    i.vote_count,
    (vu.id IS NOT NULL AND EXISTS (SELECT 1 FROM topic_votes tv WHERE tv.item_id = i.id AND tv.user_id = vu.id)) AS voted,
    CASE WHEN vu.id IS NULL THEN NULL
         WHEN rm.last_read_post_number IS NOT NULL THEN rm.last_read_post_number
         WHEN nrm.marked_at >= i.last_posted_at OR i.last_posted_at < vu.joined_at THEN i.highest_post_number
         ELSE 0 END AS last_read,
    (SELECT level FROM follows f WHERE f.user_id = vu.id AND f.target_type = 'item' AND f.target_id = i.id) AS follow_level,
    coalesce((SELECT array_agg(tg.name || '|' || tg.slug ORDER BY tg.name) FROM item_tags it JOIN tags tg ON tg.id = it.tag_id
              WHERE it.item_id = i.id), '{}') AS tag_pairs,
    coalesce((SELECT array_agg(username) FROM (
                SELECT pu.username FROM posts pp JOIN users pu ON pu.id = pp.author_id
                WHERE pp.item_id = i.id AND pp.state = 'visible' AND pp.action IS NULL
                GROUP BY pu.username ORDER BY max(pp.post_number) DESC LIMIT 5) recent), '{}') AS posters";

/// Joins for TOPIC_COLS; `$v` is the viewer's user id parameter (NULL for guests).
pub(crate) fn topic_joins(v: usize) -> String {
    format!(
        "FROM items i LEFT JOIN users a ON a.id = i.author_id LEFT JOIN users lp ON lp.id = i.last_poster_id
         LEFT JOIN users vu ON vu.id = ${v}::bigint
         LEFT JOIN read_markers rm ON rm.user_id = vu.id AND rm.item_id = i.id
         LEFT JOIN node_read_markers nrm ON nrm.user_id = vu.id AND nrm.node_id = i.node_id"
    )
}

/// Forum view ordering. Pinned topics always come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Activity,
    Newest,
    Replies,
    Views,
    Likes,
    Title,
    /// Ideas forums: most votes first.
    Votes,
}

impl Sort {
    pub fn parse(s: &str) -> Option<Sort> {
        Some(match s {
            "activity" => Sort::Activity,
            "newest" => Sort::Newest,
            "replies" => Sort::Replies,
            "views" => Sort::Views,
            "likes" => Sort::Likes,
            "title" => Sort::Title,
            "votes" => Sort::Votes,
            _ => return None,
        })
    }

    pub fn key(self) -> &'static str {
        match self {
            Sort::Activity => "activity",
            Sort::Newest => "newest",
            Sort::Replies => "replies",
            Sort::Views => "views",
            Sort::Likes => "likes",
            Sort::Title => "title",
            Sort::Votes => "votes",
        }
    }

    // Fixed strings only: never interpolate user input into ORDER BY.
    fn order_by(self) -> &'static str {
        match self {
            Sort::Activity => "i.last_posted_at DESC",
            Sort::Newest => "i.created_at DESC",
            Sort::Replies => "i.reply_count DESC, i.last_posted_at DESC",
            Sort::Views => "i.view_count DESC, i.last_posted_at DESC",
            Sort::Likes => "i.like_count DESC, i.last_posted_at DESC",
            Sort::Title => "lower(i.title) ASC",
            Sort::Votes => "i.vote_count DESC, i.last_posted_at DESC",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TopicFilter {
    pub unanswered: bool,
    pub unsolved: bool,
    /// Only topics started by this user ("my topics").
    pub author_id: Option<i64>,
    pub prefix: Option<String>,
    /// Only topics with this tag slug.
    pub tag: Option<String>,
    /// Only topics active on or after this day.
    pub after: Option<chrono::NaiveDate>,
    /// Only topics with posts the viewer has not read.
    pub unread: bool,
    /// Moderators also see unlisted topics.
    pub include_unlisted: bool,
}

pub async fn forum_topics(
    pool: &PgPool,
    node_id: i64,
    sort: Sort,
    filter: TopicFilter,
    viewer: Option<i64>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TopicRow>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TOPIC_COLS} {} WHERE i.node_id = $1 AND i.state = 'visible' AND (i.listed OR $12)
           AND (NOT $4 OR i.reply_count = 0) AND ($5::bigint IS NULL OR i.author_id = $5)
           AND ($7::text IS NULL OR i.prefix = $7)
           AND ($8::text IS NULL OR EXISTS (SELECT 1 FROM item_tags it JOIN tags tg ON tg.id = it.tag_id
                                            WHERE it.item_id = i.id AND tg.slug = $8))
           AND ($9::date IS NULL OR i.last_posted_at >= $9)
           AND (NOT $11 OR i.solved_post_number IS NULL)
           AND (NOT $10 OR (vu.id IS NOT NULL AND coalesce(rm.last_read_post_number, 0) < i.highest_post_number
                            AND NOT (rm.last_read_post_number IS NULL
                                     AND (nrm.marked_at >= i.last_posted_at OR i.last_posted_at < vu.joined_at))))
         ORDER BY i.pinned DESC, {}, i.id DESC LIMIT $2 OFFSET $3",
        topic_joins(6),
        sort.order_by()
    )))
    .bind(node_id)
    .bind(limit)
    .bind(offset)
    .bind(filter.unanswered)
    .bind(filter.author_id)
    .bind(viewer)
    .bind(&filter.prefix)
    .bind(&filter.tag)
    .bind(filter.after)
    .bind(filter.unread)
    .bind(filter.unsolved)
    .bind(filter.include_unlisted)
    .fetch_all(pool)
    .await
}

/// "Latest" stream across the forums the viewer can read (`node_id = ANY($visible)`).
pub async fn latest_topics(
    pool: &PgPool,
    visible_nodes: &[i64],
    viewer: Option<i64>,
    unread_only: bool,
    limit: i64,
) -> Result<Vec<TopicRow>, Error> {
    // The unread filter runs on the computed column, so wrap the select.
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM (SELECT {TOPIC_COLS}, i.pinned_global {} WHERE i.node_id = ANY($1) AND i.state = 'visible' AND i.listed
             AND NOT EXISTS (SELECT 1 FROM item_tags it JOIN muted_tags mt ON mt.tag_id = it.tag_id
                             WHERE it.item_id = i.id AND mt.user_id = vu.id)) t
         WHERE NOT $4 OR t.last_read < t.highest_post_number
         ORDER BY t.pinned_global DESC, t.last_posted_at DESC, t.id DESC LIMIT $2",
        topic_joins(3)
    )))
    .bind(visible_nodes)
    .bind(limit)
    .bind(viewer)
    .bind(unread_only)
    .fetch_all(pool)
    .await
}

pub async fn topic(pool: &PgPool, id: i64) -> Result<Option<TopicRow>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TOPIC_COLS} {} WHERE i.id = $1", topic_joins(2))))
        .bind(id)
        .bind(None::<i64>)
        .fetch_optional(pool)
        .await
}

/// Moderator topic state changes with an action entry in the topic flow.
pub async fn set_topic_flag(
    pool: &PgPool,
    item_id: i64,
    flag: TopicFlag,
    on: bool,
    actor_id: i64,
) -> Result<(), Error> {
    let col = match flag {
        TopicFlag::Pinned => "pinned",
        TopicFlag::Locked => "locked",
        TopicFlag::Listed => "listed",
        TopicFlag::Archived => "archived",
        TopicFlag::Featured => "featured",
    };
    let mut tx = pool.begin().await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE items SET {col} = $2, updated_at = now() WHERE id = $1")))
        .bind(item_id)
        .bind(on)
        .execute(&mut *tx)
        .await?;
    // Global pin and its expiry belong to the pin; set_pin_options applies them again afterwards.
    if matches!(flag, TopicFlag::Pinned) {
        sqlx::query("UPDATE items SET pinned_global = false, pinned_until = NULL WHERE id = $1")
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
    }
    let action = format!("{}{col}", if on { "" } else { "un" });
    let entry = NewPost { author_id: actor_id, raw: "", cooked: "", cooked_version: 0, ip: None, pending: false };
    let (post_id, _) = insert_post(&mut tx, item_id, &entry).await?;
    sqlx::query("UPDATE posts SET action = $2 WHERE id = $1").bind(post_id).bind(&action).execute(&mut *tx).await?;
    // Action entries are not authored posts.
    sqlx::query(
        "UPDATE users SET post_count = greatest(post_count - 1, 0) WHERE id = $1 AND
           (SELECT n.counts_posts FROM items i JOIN nodes n ON n.id = i.node_id WHERE i.id = $2)",
    )
    .bind(actor_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await?;
    record(&mut *tx, Entry::new(format!("topic.{action}")).by(actor_id).on(format!("item:{item_id}"))).await?;
    tx.commit().await
}

#[derive(Debug, Clone, Copy)]
pub enum TopicFlag {
    Pinned,
    Locked,
    Listed,
    Archived,
    Featured,
}

/// Topics by id in the given order, with the viewer's unread state.
pub async fn topics_by_ids(pool: &PgPool, ids: &[i64], viewer: Option<i64>) -> Result<Vec<TopicRow>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TOPIC_COLS} {} JOIN unnest($1::bigint[]) WITH ORDINALITY AS o(id, n) ON o.id = i.id ORDER BY o.n",
        topic_joins(2)
    )))
    .bind(ids)
    .bind(viewer)
    .fetch_all(pool)
    .await
}

/// A topic with the viewer's read position and follow level.
pub async fn topic_for(pool: &PgPool, id: i64, viewer: Option<i64>) -> Result<Option<TopicRow>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TOPIC_COLS} {} WHERE i.id = $1", topic_joins(2))))
        .bind(id)
        .bind(viewer)
        .fetch_optional(pool)
        .await
}

/// Pins a topic for some days and/or across the whole site.
pub async fn set_pin_options(pool: &PgPool, item: i64, days: Option<i32>, global: bool) -> Result<(), Error> {
    sqlx::query(
        "UPDATE items SET pinned_global = $3,
           pinned_until = CASE WHEN $2::int IS NULL THEN NULL ELSE now() + make_interval(days => $2) END WHERE id = $1",
    )
    .bind(item)
    .bind(days)
    .bind(global)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Ends pins whose date has passed; returns how many.
pub async fn expire_pins(pool: &PgPool) -> Result<u64, Error> {
    Ok(sqlx::query("UPDATE items SET pinned = false, pinned_global = false, pinned_until = NULL WHERE pinned AND pinned_until < now()")
        .execute(pool)
        .await?
        .rows_affected())
}

/// Copies a topic's visible posts into a new topic; returns its id. Attributed to the
/// original authors, counted in the destination forum.
pub async fn copy_topic(
    pool: &PgPool,
    from: i64,
    to_node: i64,
    title: &str,
    slug: &str,
    actor: i64,
) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO items (node_id, author_id, title, slug, last_poster_id, state, prefix)
         SELECT $2, author_id, $3, $4, last_poster_id, 'visible', prefix FROM items WHERE id = $1 RETURNING id",
    )
    .bind(from)
    .bind(to_node)
    .bind(title)
    .bind(slug)
    .fetch_one(&mut *tx)
    .await?;
    let copied = sqlx::query(
        "INSERT INTO posts (item_id, author_id, post_number, raw, cooked, cooked_version, created_at)
         SELECT $2, author_id, row_number() OVER (ORDER BY post_number), raw, cooked, cooked_version, created_at
         FROM posts WHERE item_id = $1 AND state = 'visible' AND action IS NULL",
    )
    .bind(from)
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected() as i64;
    sqlx::query(
        "UPDATE items i SET highest_post_number = $2, reply_count = greatest($2 - 1, 0),
           last_post_id = (SELECT id FROM posts WHERE item_id = i.id ORDER BY post_number DESC LIMIT 1),
           last_posted_at = coalesce((SELECT max(created_at) FROM posts WHERE item_id = i.id), now())
         WHERE id = $1",
    )
    .bind(id)
    .bind(copied as i32)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE nodes SET topic_count = topic_count + 1, post_count = post_count + $2 WHERE id = $1")
        .bind(to_node)
        .bind(copied as i32)
        .execute(&mut *tx)
        .await?;
    record(
        &mut *tx,
        Entry::new("topic.copy")
            .by(actor)
            .on(format!("item:{from}"))
            .details(serde_json::json!({ "copy": id, "to": to_node })),
    )
    .await?;
    // The copied posts are attributed to their original authors: their counts follow.
    crate::moderation::topics::recount_authors_of(&mut tx, id).await?;
    tx.commit().await?;
    Ok(id)
}

/// Changes when a topic started: its first post and the topic row (staff only).
pub async fn set_topic_date(pool: &PgPool, item: i64, at: DateTime<Utc>, actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE items SET created_at = $2 WHERE id = $1").bind(item).bind(at).execute(&mut *tx).await?;
    sqlx::query("UPDATE posts SET created_at = $2 WHERE item_id = $1 AND post_number = 1")
        .bind(item)
        .bind(at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit(pool, actor, "topic.date", format!("item:{item}"), serde_json::json!({ "created_at": at })).await
}

pub async fn add_redirect(pool: &PgPool, node: i64, item: i64, days: Option<i32>) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO topic_redirects (node_id, item_id, expires_at)
         VALUES ($1, $2, CASE WHEN $3::int IS NULL THEN NULL ELSE now() + make_interval(days => $3) END)",
    )
    .bind(node)
    .bind(item)
    .bind(days)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Live redirects in a forum: (topic id, title, slug). A topic moved back is not listed, nor is
/// one that now sits in a forum outside `readable` (the forums the viewer can read): its title
/// is not theirs to see.
pub async fn redirects(pool: &PgPool, node: i64, readable: &[i64]) -> Result<Vec<(i64, String, String)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.title, i.slug FROM topic_redirects r JOIN items i ON i.id = r.item_id
         WHERE r.node_id = $1 AND i.node_id <> $1 AND i.state = 'visible' AND i.node_id = ANY($2)
           AND (r.expires_at IS NULL OR r.expires_at > now()) ORDER BY r.id DESC LIMIT 20",
    )
    .bind(node)
    .bind(readable)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
    All,
}

impl Period {
    pub fn parse(s: &str) -> Option<Period> {
        Some(match s {
            "day" => Period::Day,
            "week" => Period::Week,
            "month" => Period::Month,
            "year" => Period::Year,
            "all" => Period::All,
            _ => return None,
        })
    }

    pub fn key(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
            Period::Year => "year",
            Period::All => "all",
        }
    }

    fn interval(self) -> &'static str {
        match self {
            Period::Day => "1 day",
            Period::Week => "7 days",
            Period::Month => "1 month",
            Period::Year => "1 year",
            Period::All => "1000 years",
        }
    }
}

/// Topic ids for a "top" list: created in the period, ranked by likes, replies
/// and views. Scored at query time; a scoring job can replace this when lists grow.
pub async fn top_ids(pool: &PgPool, visible_nodes: &[i64], period: Period, limit: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT id FROM items
         WHERE node_id = ANY($1) AND state = 'visible' AND listed AND created_at > now() - $2::interval
         ORDER BY like_count * 3 + reply_count * 2 + view_count / 10 DESC, last_posted_at DESC
         LIMIT $3",
    )
    .bind(visible_nodes)
    .bind(period.interval())
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// A held topic goes public.
pub async fn publish_item(pool: &PgPool, item: i64) -> Result<(), Error> {
    // Only what is still held: a moderator may have rejected or deleted it in the meantime.
    let post: Option<i64> = sqlx::query_scalar(
        "SELECT p.id FROM posts p JOIN items i ON i.id = p.item_id
         WHERE p.item_id = $1 AND p.post_number = 1 AND p.state = 'pending' AND i.state <> 'deleted'",
    )
    .bind(item)
    .fetch_optional(pool)
    .await?;
    let Some(p) = post else { return Ok(()) };
    crate::forum::posts::restore_to_visible(pool, p).await?;
    // It is new now: it sorts by its publish time, not by when it was written.
    sqlx::query("UPDATE items SET last_posted_at = now() WHERE id = $1").bind(item).execute(pool).await?;
    // The hold is over, so its review item must not linger (or later delete a live topic).
    sqlx::query(
        "UPDATE reviewables SET status = 'approved', resolved_at = now()
         WHERE kind = 'pending_post' AND target_type = 'post' AND target_id = $1 AND status = 'pending'",
    )
    .bind(p)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The queue of a member's scheduled topics.
pub async fn scheduled_topics(pool: &PgPool, author: i64) -> Result<Vec<(i64, String, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.title, t.run_at FROM item_timers t JOIN items i ON i.id = t.item_id
         WHERE t.action = 'publish' AND i.author_id = $1 ORDER BY t.run_at",
    )
    .bind(author)
    .fetch_all(pool)
    .await
}

/// The starter of a visible topic and its forum.
pub async fn topic_owner(pool: &PgPool, item: i64) -> Result<Option<(i64, i64)>, Error> {
    sqlx::query_as(
        "SELECT p.author_id, i.node_id FROM items i JOIN posts p ON p.item_id = i.id AND p.post_number = 1
         WHERE i.id = $1 AND i.state = 'visible' AND p.author_id IS NOT NULL",
    )
    .bind(item)
    .fetch_optional(pool)
    .await
}

/// The topic behind an emailed unsubscribe link.
pub async fn token_topic(pool: &PgPool, token: &str) -> Result<Option<(i64, i64)>, Error> {
    sqlx::query_as("SELECT user_id, item_id FROM unsubscribe_tokens WHERE token = $1 AND item_id IS NOT NULL")
        .bind(token)
        .fetch_optional(pool)
        .await
}

crate::sql::set_col!(set_prefix, "items", "prefix", i64, Option<&str>);

/// A member's votes that count toward their limit: on open, visible topics, other than `$2`.
const VOTES_USED: &str = "SELECT count(*) FROM topic_votes v JOIN items i ON i.id = v.item_id
     WHERE v.user_id = $1 AND v.item_id <> $2 AND NOT i.locked AND i.state = 'visible'";

/// How many of a member's votes are in use (see `vote_topic`).
pub async fn votes_used(pool: &PgPool, user: i64) -> Result<i64, Error> {
    sqlx::query_scalar(VOTES_USED).bind(user).bind(0_i64).fetch_one(pool).await
}

/// Who voted for a topic, latest first: (id, username), at most `limit`.
pub async fn topic_voters(pool: &PgPool, item: i64, limit: i64) -> Result<Vec<(i64, String)>, Error> {
    sqlx::query_as(
        "SELECT u.id, u.username FROM topic_votes v JOIN users u ON u.id = v.user_id
         WHERE v.item_id = $1 ORDER BY v.created_at DESC LIMIT $2",
    )
    .bind(item)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// A topic a member voted for, as their profile lists it.
#[derive(Debug, Clone, FromRow)]
pub struct VotedTopic {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub vote_count: i32,
    pub locked: bool,
    pub voted_at: DateTime<Utc>,
}

/// The topics a member voted for, in forums the viewer can read; open ones first, then newest vote.
pub async fn voted_topics(
    pool: &PgPool,
    user: i64,
    visible_nodes: &[i64],
    limit: i64,
) -> Result<Vec<VotedTopic>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.slug, i.title, i.vote_count, i.locked, v.created_at AS voted_at
         FROM topic_votes v JOIN items i ON i.id = v.item_id
         WHERE v.user_id = $1 AND i.state = 'visible' AND i.listed AND i.node_id = ANY($2)
         ORDER BY i.locked, v.created_at DESC LIMIT $3",
    )
    .bind(user)
    .bind(visible_nodes)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Votes for a topic in an ideas forum, or takes the vote back. Only votes on open topics count
/// toward `limit` (0 is no limit), so closing a topic gives its votes back. None when the member
/// has no vote left; otherwise the topic's new total.
pub async fn vote_topic(pool: &PgPool, item: i64, user: i64, on: bool, limit: i64) -> Result<Option<i32>, Error> {
    let mut tx = pool.begin().await?;
    // One member's votes are counted and spent one at a time.
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE").bind(user).execute(&mut *tx).await?;
    if on {
        if limit > 0 {
            let used: i64 = sqlx::query_scalar(VOTES_USED).bind(user).bind(item).fetch_one(&mut *tx).await?;
            if used >= limit {
                return Ok(None);
            }
        }
        sqlx::query("INSERT INTO topic_votes (item_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(item)
            .bind(user)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("DELETE FROM topic_votes WHERE item_id = $1 AND user_id = $2")
            .bind(item)
            .bind(user)
            .execute(&mut *tx)
            .await?;
    }
    let total: i32 = sqlx::query_scalar(
        "UPDATE items SET vote_count = (SELECT count(*) FROM topic_votes WHERE item_id = $1) WHERE id = $1
         RETURNING vote_count",
    )
    .bind(item)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(total))
}

/// What `claim_invite` decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Invite {
    Allowed,
    /// The sender invited too many people in the last hour.
    TooMany,
    /// This member was already invited to this topic.
    Already,
}

/// Records a topic invitation (the log is the memory), refusing a repeat for the same member and
/// topic and more than 20 invitations an hour from one sender.
pub async fn claim_invite(pool: &PgPool, actor: i64, target: i64, topic: i64) -> Result<Invite, Error> {
    let mut tx = pool.begin().await?;
    // Serialises one sender's invitations, so two requests cannot both pass the checks.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('topic-invite:' || ($1::bigint)::text))")
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    let key = format!("topic-invite:{topic}:{target}");
    let already: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM audit_log WHERE action = 'topic.invite' AND target = $1)")
            .bind(&key)
            .fetch_one(&mut *tx)
            .await?;
    if already {
        return Ok(Invite::Already);
    }
    let recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'topic.invite' AND actor_id = $1 AND created_at > now() - interval '1 hour'",
    )
    .bind(actor)
    .fetch_one(&mut *tx)
    .await?;
    if recent >= 20 {
        return Ok(Invite::TooMany);
    }
    record(&mut *tx, Entry::new("topic.invite").by(actor).on(key.clone())).await?;
    tx.commit().await?;
    Ok(Invite::Allowed)
}
