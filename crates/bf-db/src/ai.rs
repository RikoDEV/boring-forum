//! AI features: the usage log, cached summaries, post labels, topic embeddings
//! and bot personas.

use chrono::{DateTime, NaiveDate, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

// ---- Usage ----

#[allow(clippy::too_many_arguments)]
pub async fn log_usage(
    pool: &PgPool,
    feature: &str,
    model: &str,
    user: Option<i64>,
    prompt: i32,
    completion: i32,
    ok: bool,
    error: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO ai_usage (feature, model, user_id, prompt_tokens, completion_tokens, ok, error)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(feature)
    .bind(model)
    .bind(user)
    .bind(prompt)
    .bind(completion)
    .bind(ok)
    .bind(error.map(|e| e.chars().take(300).collect::<String>()))
    .execute(pool)
    .await
    .map(|_| ())
}

/// Tokens used since the first of this month (UTC), by every feature.
pub async fn tokens_this_month(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(prompt_tokens + completion_tokens), 0)::bigint FROM ai_usage
         WHERE created_at >= date_trunc('month', now())",
    )
    .fetch_one(pool)
    .await
}

/// Requests a member made in the last 24 hours.
pub async fn requests_today(pool: &PgPool, user: i64) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM ai_usage WHERE user_id = $1 AND created_at > now() - interval '1 day'")
        .bind(user)
        .fetch_one(pool)
        .await
}

#[derive(Debug, Clone, FromRow)]
pub struct UsageRow {
    pub day: NaiveDate,
    pub feature: String,
    pub requests: i64,
    pub failures: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

pub async fn usage_report(pool: &PgPool, days: i32) -> Result<Vec<UsageRow>, Error> {
    sqlx::query_as(
        "SELECT created_at::date AS day, feature, count(*) AS requests, count(*) FILTER (WHERE NOT ok) AS failures,
                COALESCE(sum(prompt_tokens), 0)::bigint AS prompt_tokens,
                COALESCE(sum(completion_tokens), 0)::bigint AS completion_tokens
         FROM ai_usage WHERE created_at > now() - make_interval(days => $1)
         GROUP BY 1, 2 ORDER BY 1 DESC, 2",
    )
    .bind(days)
    .fetch_all(pool)
    .await
}

pub async fn recent_errors(pool: &PgPool) -> Result<Vec<(DateTime<Utc>, String, String)>, Error> {
    sqlx::query_as(
        "SELECT created_at, feature, COALESCE(error, '') FROM ai_usage WHERE NOT ok ORDER BY id DESC LIMIT 10",
    )
    .fetch_all(pool)
    .await
}

/// Old rows go with the daily housekeeping.
pub async fn purge_usage(pool: &PgPool, days: i32) -> Result<u64, Error> {
    Ok(sqlx::query("DELETE FROM ai_usage WHERE created_at < now() - make_interval(days => $1)")
        .bind(days)
        .execute(pool)
        .await?
        .rows_affected())
}

// ---- Summaries ----

#[derive(Debug, Clone, FromRow)]
pub struct Summary {
    pub up_to_post: i32,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

pub async fn summary(pool: &PgPool, item: i64, kind: &str, locale: &str) -> Result<Option<Summary>, Error> {
    sqlx::query_as(
        "SELECT up_to_post, body, created_at FROM ai_summaries WHERE item_id = $1 AND kind = $2 AND locale = $3",
    )
    .bind(item)
    .bind(kind)
    .bind(locale)
    .fetch_optional(pool)
    .await
}

pub async fn save_summary(
    pool: &PgPool,
    item: i64,
    kind: &str,
    locale: &str,
    up_to: i32,
    body: &str,
    model: &str,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO ai_summaries (item_id, kind, locale, up_to_post, body, model) VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (item_id, kind, locale)
         DO UPDATE SET up_to_post = $4, body = $5, model = $6, created_at = now()",
    )
    .bind(item)
    .bind(kind)
    .bind(locale)
    .bind(up_to)
    .bind(body)
    .bind(model)
    .execute(pool)
    .await
    .map(|_| ())
}

/// A topic's visible posts after `after` as (number, author, raw), oldest first.
pub async fn topic_posts(
    pool: &PgPool,
    item: i64,
    after: i32,
    limit: i64,
) -> Result<Vec<(i32, String, String)>, Error> {
    sqlx::query_as(
        "SELECT p.post_number, COALESCE(u.username, 'deleted'), bf_strip_hidden(p.raw) FROM posts p
         LEFT JOIN users u ON u.id = p.author_id
         WHERE p.item_id = $1 AND p.state = 'visible' AND p.post_number > $2
         ORDER BY p.post_number LIMIT $3",
    )
    .bind(item)
    .bind(after)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Visible posts a member has not read yet, or the latest ones when they have no record.
pub async fn last_read(pool: &PgPool, item: i64, user: i64) -> Result<i32, Error> {
    sqlx::query_scalar(
        "SELECT coalesce((SELECT last_read_post_number FROM read_markers WHERE user_id = $1 AND item_id = $2), 0)",
    )
    .bind(user)
    .bind(item)
    .fetch_one(pool)
    .await
}

/// Busy topics that have no gist (or an old one), most active first: (item, node, title, posts, gist_up_to).
pub async fn gist_candidates(
    pool: &PgPool,
    locale: &str,
    min_replies: i32,
    limit: i64,
) -> Result<Vec<(i64, i64, String, i32)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.node_id, i.title, i.reply_count FROM items i
         LEFT JOIN ai_summaries s ON s.item_id = i.id AND s.kind = 'gist' AND s.locale = $1
         WHERE i.kind = 'topic' AND i.state = 'visible' AND i.reply_count >= $2
           AND (s.item_id IS NULL OR i.reply_count >= s.up_to_post + GREATEST(10, s.up_to_post / 2))
         ORDER BY i.last_posted_at DESC LIMIT $3",
    )
    .bind(locale)
    .bind(min_replies)
    .bind(limit)
    .fetch_all(pool)
    .await
}

// ---- Labels ----

pub async fn save_labels(
    pool: &PgPool,
    post: i64,
    spam: i16,
    toxicity: i16,
    sentiment: &str,
    emotion: &str,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO ai_post_labels (post_id, spam, toxicity, sentiment, emotion) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (post_id) DO UPDATE SET spam = $2, toxicity = $3, sentiment = $4, emotion = $5, created_at = now()",
    )
    .bind(post)
    .bind(spam)
    .bind(toxicity)
    .bind(sentiment)
    .bind(emotion.chars().take(30).collect::<String>())
    .execute(pool)
    .await
    .map(|_| ())
}

#[derive(Debug, Clone, FromRow)]
pub struct MoodRow {
    pub day: NaiveDate,
    pub positive: i64,
    pub neutral: i64,
    pub negative: i64,
    pub toxic: i64,
    pub spam: i64,
}

pub async fn mood_report(pool: &PgPool, days: i32, toxic_at: i16, spam_at: i16) -> Result<Vec<MoodRow>, Error> {
    sqlx::query_as(
        "SELECT created_at::date AS day,
                count(*) FILTER (WHERE sentiment = 'positive') AS positive,
                count(*) FILTER (WHERE sentiment = 'neutral') AS neutral,
                count(*) FILTER (WHERE sentiment = 'negative') AS negative,
                count(*) FILTER (WHERE toxicity >= $2) AS toxic,
                count(*) FILTER (WHERE spam >= $3) AS spam
         FROM ai_post_labels WHERE created_at > now() - make_interval(days => $1)
         GROUP BY 1 ORDER BY 1 DESC",
    )
    .bind(days)
    .bind(toxic_at)
    .bind(spam_at)
    .fetch_all(pool)
    .await
}

/// The most common emotions lately, for the report.
pub async fn top_emotions(pool: &PgPool, days: i32) -> Result<Vec<(String, i64)>, Error> {
    sqlx::query_as(
        "SELECT emotion, count(*) FROM ai_post_labels
         WHERE emotion <> '' AND created_at > now() - make_interval(days => $1)
         GROUP BY 1 ORDER BY 2 DESC LIMIT 8",
    )
    .bind(days)
    .fetch_all(pool)
    .await
}

/// Posts that look bad, newest first: (post, topic title, author, spam, toxicity).
pub async fn flagged_posts(
    pool: &PgPool,
    toxic_at: i16,
    spam_at: i16,
) -> Result<Vec<(i64, i64, String, String, i16, i16)>, Error> {
    sqlx::query_as(
        "SELECT l.post_id, i.id, i.title, COALESCE(u.username, 'deleted'), l.spam, l.toxicity
         FROM ai_post_labels l JOIN posts p ON p.id = l.post_id JOIN items i ON i.id = p.item_id
         LEFT JOIN users u ON u.id = p.author_id
         WHERE (l.spam >= $2 OR l.toxicity >= $1) AND p.state <> 'deleted'
         ORDER BY l.created_at DESC LIMIT 25",
    )
    .bind(toxic_at)
    .bind(spam_at)
    .fetch_all(pool)
    .await
}

// ---- Embeddings ----

#[derive(Debug, Clone, FromRow)]
pub struct StoredVector {
    pub item_id: i64,
    pub node_id: i64,
    pub vec: Vec<u8>,
}

pub async fn upsert_embedding(
    pool: &PgPool,
    item: i64,
    node: i64,
    model: &str,
    vec: &[u8],
    dims: i32,
    hash: &str,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO ai_embeddings (item_id, node_id, model, dims, vec, text_hash) VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (item_id) DO UPDATE SET node_id = $2, model = $3, dims = $4, vec = $5, text_hash = $6, updated_at = now()",
    )
    .bind(item)
    .bind(node)
    .bind(model)
    .bind(dims)
    .bind(vec)
    .bind(hash)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Every stored vector of this model, for the in-memory index. The forum is the topic's current
/// one, not the one it was in when the vector was made: a topic may have been moved since.
pub async fn all_embeddings(pool: &PgPool, model: &str, limit: i64) -> Result<Vec<StoredVector>, Error> {
    sqlx::query_as(
        "SELECT e.item_id, i.node_id, e.vec FROM ai_embeddings e JOIN items i ON i.id = e.item_id
         WHERE e.model = $1 AND i.state = 'visible' ORDER BY i.last_posted_at DESC LIMIT $2",
    )
    .bind(model)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Topics whose embedding is missing, was made by another model, or is older than the topic's
/// text: (item, node, title, opening post, hash now).
pub async fn embedding_work(
    pool: &PgPool,
    model: &str,
    nodes: Option<&[i64]>,
    limit: i64,
) -> Result<Vec<(i64, i64, String, String)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.node_id, i.title, COALESCE(bf_strip_hidden(p.raw), '') FROM items i
         LEFT JOIN posts p ON p.item_id = i.id AND p.post_number = 1
         LEFT JOIN ai_embeddings e ON e.item_id = i.id
         WHERE i.kind = 'topic' AND i.state = 'visible' AND ($3::bigint[] IS NULL OR i.node_id = ANY($3))
           AND (e.item_id IS NULL OR e.model <> $1 OR e.updated_at < i.last_posted_at - interval '7 days')
         ORDER BY i.last_posted_at DESC LIMIT $2",
    )
    .bind(model)
    .bind(limit)
    .bind(nodes)
    .fetch_all(pool)
    .await
}

pub async fn embedding_count(pool: &PgPool) -> Result<(i64, i64), Error> {
    let done: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_embeddings").fetch_one(pool).await?;
    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE kind = 'topic' AND state = 'visible'")
        .fetch_one(pool)
        .await?;
    Ok((done, all))
}

pub async fn drop_embeddings(pool: &PgPool) -> Result<u64, Error> {
    Ok(sqlx::query("DELETE FROM ai_embeddings").execute(pool).await?.rows_affected())
}

// ---- Personas ----

#[derive(Debug, Clone, FromRow, serde::Serialize, serde::Deserialize)]
pub struct Persona {
    #[serde(default)]
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub system_prompt: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub model: String,
    #[serde(default = "yes")]
    pub use_search: bool,
    #[serde(default = "tokens")]
    pub max_tokens: i32,
    #[serde(default)]
    pub group_ids: Vec<i64>,
}

fn yes() -> bool {
    true
}

fn tokens() -> i32 {
    800
}

pub async fn personas(pool: &PgPool) -> Result<Vec<Persona>, Error> {
    sqlx::query_as(
        "SELECT id, slug, name, system_prompt, enabled, model, use_search, max_tokens, group_ids FROM ai_personas ORDER BY id",
    ).fetch_all(pool).await
}

pub async fn persona_by_slug(pool: &PgPool, slug: &str) -> Result<Option<Persona>, Error> {
    sqlx::query_as(
        "SELECT id, slug, name, system_prompt, enabled, model, use_search, max_tokens, group_ids FROM ai_personas
         WHERE slug = $1 AND enabled",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await
}

pub async fn save_persona(pool: &PgPool, id: Option<i64>, p: &Persona) -> Result<Option<i64>, Error> {
    match id {
        None => {
            sqlx::query_scalar(
                "INSERT INTO ai_personas (slug, name, system_prompt, enabled, model, use_search, max_tokens, group_ids)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
            )
            .bind(&p.slug)
            .bind(&p.name)
            .bind(&p.system_prompt)
            .bind(p.enabled)
            .bind(&p.model)
            .bind(p.use_search)
            .bind(p.max_tokens)
            .bind(&p.group_ids)
            .fetch_optional(pool)
            .await
        }
        Some(id) => {
            sqlx::query_scalar(
                "UPDATE ai_personas SET slug = $2, name = $3, system_prompt = $4, enabled = $5, model = $6,
                   use_search = $7, max_tokens = $8, group_ids = $9 WHERE id = $1 RETURNING id",
            )
            .bind(id)
            .bind(&p.slug)
            .bind(&p.name)
            .bind(&p.system_prompt)
            .bind(p.enabled)
            .bind(&p.model)
            .bind(p.use_search)
            .bind(p.max_tokens)
            .bind(&p.group_ids)
            .fetch_optional(pool)
            .await
        }
    }
}

crate::sql::delete_by_id!(delete_persona, "ai_personas");

// ---- Looking topics up ----

/// Visible topics by id: (id, slug, title, node, author, created, opening of the first post).
#[allow(clippy::type_complexity)]
pub async fn topic_cards(
    pool: &PgPool,
    ids: &[i64],
) -> Result<Vec<(i64, String, String, i64, Option<String>, DateTime<Utc>, String)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.slug, i.title, i.node_id, u.username, i.created_at,
                left(COALESCE(bf_strip_hidden(p.raw), ''), 220)
         FROM items i LEFT JOIN users u ON u.id = i.author_id
         LEFT JOIN posts p ON p.item_id = i.id AND p.post_number = 1
         WHERE i.id = ANY($1) AND i.state = 'visible' AND i.listed",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// The account a persona posts as: `ai-<slug>`, with no way to sign in (no password, a reserved
/// mail domain) and never an admin. Created on first use.
pub async fn ensure_persona_user(pool: &PgPool, slug: &str) -> Result<(i64, String), Error> {
    let name = format!("ai-{slug}");
    let email = format!("{name}@plugins.invalid");
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE email_lower = lower($1)")
        .bind(&email)
        .fetch_optional(pool)
        .await?;
    let id = if let Some(id) = existing {
        id
    } else {
        let mut tx = pool.begin().await?;
        let (id, made) = crate::accounts::users::insert_user(
            &mut tx,
            &crate::accounts::users::NewUser {
                username: &name,
                email: &email,
                password_hash: None,
                display_name: None,
                primary_group: Some(2),
                joined_at: None,
                verified: true,
                kind: crate::accounts::users::AccountKind::Bot,
                fit_name: true,
            },
        )
        .await?;
        tx.commit().await?;
        return Ok((id.get(), made));
    };
    Ok((id, name))
}

/// Whether `slug` is free as `ai-<slug>` among members (another member may already own the name).
pub async fn persona_name_taken(pool: &PgPool, slug: &str) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users WHERE username_lower = lower($1) AND email_lower <> lower($2))",
    )
    .bind(format!("ai-{slug}"))
    .bind(format!("ai-{slug}@plugins.invalid"))
    .fetch_one(pool)
    .await
}

/// What happened lately, for a report: (new topics, new posts, new members) and the busiest
/// topics as (title, new posts, forum) in the given forums.
pub async fn activity(
    pool: &PgPool,
    days: i32,
    nodes: &[i64],
) -> Result<((i64, i64, i64), Vec<(i64, String, i64, i64)>), Error> {
    let topics: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM items WHERE kind = 'topic' AND state = 'visible' AND node_id = ANY($2)
           AND created_at > now() - make_interval(days => $1)",
    )
    .bind(days)
    .bind(nodes)
    .fetch_one(pool)
    .await?;
    let posts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM posts p JOIN items i ON i.id = p.item_id
         WHERE p.state = 'visible' AND i.node_id = ANY($2) AND p.created_at > now() - make_interval(days => $1)",
    )
    .bind(days)
    .bind(nodes)
    .fetch_one(pool)
    .await?;
    let members: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE joined_at > now() - make_interval(days => $1)")
            .bind(days)
            .fetch_one(pool)
            .await?;
    let busiest = sqlx::query_as(
        "SELECT i.id, i.title, count(p.id) AS fresh, i.node_id FROM items i
         JOIN posts p ON p.item_id = i.id AND p.state = 'visible' AND p.created_at > now() - make_interval(days => $1)
         WHERE i.state = 'visible' AND i.node_id = ANY($2) GROUP BY i.id ORDER BY 3 DESC, i.id DESC LIMIT 15",
    )
    .bind(days)
    .bind(nodes)
    .fetch_all(pool)
    .await?;
    Ok(((topics, posts, members), busiest))
}
