//! Likes and reactions on posts.

pub use sqlx::Error;
use sqlx::PgPool;

/// The id of the plain "like" reaction (the row flagged `is_like`).
pub async fn like_reaction_id(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT id FROM reactions WHERE is_like").fetch_one(pool).await
}

/// Toggles the viewer's like. Returns (liked now, new count).
pub async fn toggle_like(pool: &PgPool, post_id: i64, user_id: i64) -> Result<(bool, i32), Error> {
    Ok(toggle_like_capped(pool, post_id, user_id, None).await?.unwrap_or((false, 0)))
}

/// [`toggle_like`] that refuses (`None`) to add a reaction when the member already made
/// `daily_cap` in the last day. The count and the insert happen under one per-member lock.
pub async fn toggle_like_capped(
    pool: &PgPool,
    post_id: i64,
    user_id: i64,
    daily_cap: Option<i64>,
) -> Result<Option<(bool, i32)>, Error> {
    let current: Option<i64> =
        sqlx::query_scalar("SELECT reaction_id FROM post_likes WHERE post_id = $1 AND user_id = $2")
            .bind(post_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    let liked = current.is_none();
    let like = if liked { Some(like_reaction_id(pool).await?) } else { None };
    Ok(set_reaction_capped(pool, post_id, user_id, like, daily_cap).await?.map(|count| (liked, count)))
}

/// Sets, changes or (with `None`) removes one member's reaction. Counters and the author's
/// reputation move in the same transaction. Returns the post's new reaction count.
pub async fn set_reaction(pool: &PgPool, post_id: i64, user_id: i64, reaction: Option<i64>) -> Result<i32, Error> {
    Ok(set_reaction_capped(pool, post_id, user_id, reaction, None).await?.unwrap_or(0))
}

/// [`set_reaction`] with the day's allowance: a new reaction (none of this member's on the post
/// yet) is refused with `None` when they already made `daily_cap` in the last day. Counting and
/// inserting are serialised per member, so two requests at once cannot both pass the check.
pub async fn set_reaction_capped(
    pool: &PgPool,
    post_id: i64,
    user_id: i64,
    reaction: Option<i64>,
    daily_cap: Option<i64>,
) -> Result<Option<i32>, Error> {
    let mut tx = pool.begin().await?;
    if daily_cap.is_some() {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('reactions_per_day:' || $1::text))")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }
    let old: Option<i16> = sqlx::query_scalar(
        "DELETE FROM post_likes l USING reactions r WHERE l.post_id = $1 AND l.user_id = $2 AND r.id = l.reaction_id
         RETURNING r.score",
    )
    .bind(post_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let mut new: Option<i16> = None;
    if let Some(reaction) = reaction {
        if let (None, Some(cap)) = (old, daily_cap) {
            let today: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM post_likes WHERE user_id = $1 AND created_at > now() - interval '1 day'",
            )
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
            if today >= cap {
                return Ok(None);
            }
        }
        new = Some(
            sqlx::query_scalar(
                "WITH i AS (INSERT INTO post_likes (post_id, user_id, reaction_id) VALUES ($1, $2, $3) RETURNING reaction_id)
                 SELECT r.score FROM reactions r JOIN i ON i.reaction_id = r.id",
            )
            .bind(post_id)
            .bind(user_id)
            .bind(reaction)
            .fetch_one(&mut *tx)
            .await?,
        );
    }
    let delta = i32::from(new.is_some()) - i32::from(old.is_some());
    let count: i32 =
        sqlx::query_scalar("UPDATE posts SET like_count = like_count + $2 WHERE id = $1 RETURNING like_count")
            .bind(post_id)
            .bind(delta)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query("UPDATE items SET like_count = like_count + $2 WHERE id = (SELECT item_id FROM posts WHERE id = $1)")
        .bind(post_id)
        .bind(delta)
        .execute(&mut *tx)
        .await?;
    let score = i64::from(new.unwrap_or(0)) - i64::from(old.unwrap_or(0));
    sqlx::query("UPDATE users SET reputation = reputation + $2 WHERE id = (SELECT author_id FROM posts WHERE id = $1)")
        .bind(post_id)
        .bind(score)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(count))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Reaction {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub score: i16,
    pub enabled: bool,
    pub position: i32,
    /// The plugin that declared this reaction: it is managed there.
    #[serde(default, skip_deserializing)]
    pub plugin: Option<String>,
    /// The plain "like": the like button is built on it, so it can be switched off but not deleted.
    #[serde(default, skip_deserializing)]
    pub is_like: bool,
}

pub async fn reactions(pool: &PgPool) -> Result<Vec<Reaction>, Error> {
    sqlx::query_as(
        "SELECT id, name, icon, score, enabled, position, plugin, is_like FROM reactions ORDER BY position, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn create_reaction(pool: &PgPool, r: &Reaction) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO reactions (name, icon, score, enabled, position)
         VALUES ($1, $2, $3, $4, (SELECT coalesce(max(position), 0) + 1 FROM reactions)) RETURNING id",
    )
    .bind(&r.name)
    .bind(&r.icon)
    .bind(r.score)
    .bind(r.enabled)
    .fetch_one(pool)
    .await
}

/// Scores are not rewritten into past reputation: a new score counts from now on.
pub async fn update_reaction(pool: &PgPool, id: i64, r: &Reaction) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE reactions SET name = $2, icon = $3, score = $4, enabled = $5 WHERE id = $1 AND plugin IS NULL",
    )
    .bind(id)
    .bind(&r.name)
    .bind(&r.icon)
    .bind(r.score)
    .bind(r.enabled)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// Puts the reactions in the order given, first to last; the order members see them in.
pub async fn reorder_reactions(pool: &PgPool, ids: &[i64]) -> Result<(), Error> {
    sqlx::query(
        "UPDATE reactions SET position = (t.ord - 1)::int FROM unnest($1::bigint[]) WITH ORDINALITY AS t(id, ord)
         WHERE reactions.id = t.id",
    )
    .bind(ids)
    .execute(pool)
    .await?;
    Ok(())
}

/// What deleting a reaction came to.
#[derive(Debug, PartialEq, Eq)]
pub enum ReactionDelete {
    Deleted,
    NotFound,
    /// The like button is built on this one.
    IsLike,
    /// A plugin declared it: it goes with the plugin.
    FromPlugin,
}

/// Deletes a reaction and every reaction members gave with it. The posts' counters follow; the
/// reputation those reactions earned stays (a score is never rewritten into the past).
pub async fn delete_reaction(pool: &PgPool, id: i64) -> Result<ReactionDelete, Error> {
    let mut tx = pool.begin().await?;
    let row: Option<(bool, Option<String>)> =
        sqlx::query_as("SELECT is_like, plugin FROM reactions WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    match row {
        None => return Ok(ReactionDelete::NotFound),
        Some((true, _)) => return Ok(ReactionDelete::IsLike),
        Some((false, Some(_))) => return Ok(ReactionDelete::FromPlugin),
        Some((false, None)) => {}
    }
    sqlx::query(
        "WITH gone AS (DELETE FROM post_likes WHERE reaction_id = $1 RETURNING post_id),
              per AS (SELECT post_id, count(*)::int AS n FROM gone GROUP BY post_id),
              moved AS (UPDATE posts SET like_count = greatest(0, like_count - per.n)
                        FROM per WHERE posts.id = per.post_id RETURNING posts.item_id, per.n)
         UPDATE items SET like_count = greatest(0, like_count - s.n)
         FROM (SELECT item_id, sum(n)::int AS n FROM moved GROUP BY item_id) s WHERE items.id = s.item_id",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM reactions WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(ReactionDelete::Deleted)
}

/// When this member reacted to this post, if they have.
pub async fn reacted_at(
    pool: &PgPool,
    post_id: i64,
    user_id: i64,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, Error> {
    sqlx::query_scalar("SELECT created_at FROM post_likes WHERE post_id = $1 AND user_id = $2")
        .bind(post_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
}

pub async fn reactions_today(pool: &PgPool, user_id: i64) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM post_likes WHERE user_id = $1 AND created_at > now() - interval '1 day'")
        .bind(user_id)
        .fetch_one(pool)
        .await
}

/// How a post's reactions stand: for each reaction in use its icon and how many chose it (in the order the
/// site lists them), and the reaction this member chose, if any.
pub async fn summary(
    pool: &PgPool,
    post_id: i64,
    user_id: i64,
) -> Result<(Vec<(i64, String, i64)>, Option<i64>), Error> {
    let counts: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT r.id, r.icon, count(*) FROM post_likes l JOIN reactions r ON r.id = l.reaction_id
         WHERE l.post_id = $1 GROUP BY r.id, r.icon, r.position ORDER BY r.position, r.id",
    )
    .bind(post_id)
    .fetch_all(pool)
    .await?;
    let mine = sqlx::query_scalar("SELECT reaction_id FROM post_likes WHERE post_id = $1 AND user_id = $2")
        .bind(post_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok((counts, mine))
}

/// One member's reaction to a post.
#[derive(Debug, sqlx::FromRow)]
pub struct Reactor {
    pub reaction_id: i64,
    pub icon: String,
    pub name: String,
    pub username: String,
    pub at: chrono::DateTime<chrono::Utc>,
}

/// Who reacted to a post, grouped by reaction in the order the site lists them, the newest first
/// within each.
pub async fn who_reacted(pool: &PgPool, post_id: i64) -> Result<Vec<Reactor>, Error> {
    sqlx::query_as(
        "SELECT r.id AS reaction_id, r.icon, r.name, u.username, l.created_at AS at FROM post_likes l
         JOIN reactions r ON r.id = l.reaction_id JOIN users u ON u.id = l.user_id
         WHERE l.post_id = $1 ORDER BY r.position, r.id, l.created_at DESC LIMIT 500",
    )
    .bind(post_id)
    .fetch_all(pool)
    .await
}

/// Posts on this page the user has liked.
pub async fn liked_posts(pool: &PgPool, user_id: i64, post_ids: &[i64]) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT post_id FROM post_likes WHERE user_id = $1 AND post_id = ANY($2)")
        .bind(user_id)
        .bind(post_ids)
        .fetch_all(pool)
        .await
}
