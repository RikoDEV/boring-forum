//! The forum tree: categories, forums and links, their permissions and read markers.

use bf_core::ids::{ForumId, GroupId};
use bf_core::perms::{NodeGrants, Perms};
use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;

#[derive(Debug, Clone, FromRow)]
pub struct Node {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub kind: String,
    pub slug: String,
    pub title: String,
    pub description_html: String,
    /// The Markdown the description was written in: what a translation of it is looked up by.
    pub description_md: String,
    pub redirect_url: Option<String>,
    pub position: i32,
    pub topic_count: i32,
    pub post_count: i32,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub default_sort: Option<String>,
    pub seo_title: Option<String>,
    pub seo_description: Option<String>,
    pub list_style: String,
    pub rules_html: Option<String>,
    pub prefixes: Vec<String>,
    pub template_md: Option<String>,
    pub icon: Option<String>,
    pub icon_image: Option<String>,
    pub theme_id: Option<i64>,
    pub approve_mode: String,
    pub min_trust_level: i16,
    pub require_tag: bool,
    pub clicks: i32,
    pub counts_posts: bool,
    /// `discussion`, `qa` (answers voted best first) or `ideas` (topics voted for).
    pub mode: String,
    /// Topics close after this many days without a reply.
    pub auto_close_days: Option<i32>,
    /// Moved topics left a link here; checked so most forums skip that query.
    pub has_redirects: bool,
    /// Uses its parent's permissions (and so on up) instead of grants of its own.
    pub inherit_perms: bool,
}

/// The node whose grants apply to `id`: itself, or the nearest ancestor up the chain of nodes that
/// inherit. A node without a parent never inherits.
pub fn grants_node(nodes: &[Node], id: i64) -> i64 {
    let mut at = id;
    // The depth bound only guards against a corrupt cycle.
    for _ in 0..=nodes.len() {
        let Some(n) = nodes.iter().find(|n| n.id == at) else { return at };
        match n.parent_id {
            Some(p) if n.inherit_perms => at = p,
            _ => return at,
        }
    }
    at
}

/// The whole tree in one query; forums are few, so callers filter by permission in memory.
pub async fn nodes(pool: &PgPool) -> Result<Vec<Node>, Error> {
    sqlx::query_as(
        "SELECT id, parent_id, kind, slug, title, description_html, coalesce(description_md, '') AS description_md, redirect_url, position,
                topic_count, post_count, last_activity_at, default_sort, seo_title, seo_description, list_style,
                settings->>'rules_html' AS rules_html, settings->>'template_md' AS template_md, settings->>'icon' AS icon, settings->>'icon_image' AS icon_image, theme_id, approve_mode, min_trust_level, require_tag, clicks, counts_posts, mode, auto_close_days,
                EXISTS (SELECT 1 FROM topic_redirects r WHERE r.node_id = nodes.id) AS has_redirects, inherit_perms,
                ARRAY(SELECT jsonb_array_elements_text(CASE WHEN jsonb_typeof(settings->'prefixes') = 'array' THEN settings->'prefixes' ELSE '[]'::jsonb END)) AS prefixes
         FROM nodes ORDER BY position, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn node_grants(pool: &PgPool) -> Result<HashMap<ForumId, NodeGrants>, Error> {
    let rows: Vec<(ForumId, GroupId, i32)> =
        sqlx::query_as("SELECT node_id, group_id, perms FROM node_permissions").fetch_all(pool).await?;
    let mut out: HashMap<ForumId, NodeGrants> = HashMap::new();
    for (node, group, perms) in rows {
        out.entry(node).or_default().insert(group, Perms(perms as u32));
    }
    Ok(out)
}

pub async fn create_node(
    pool: &PgPool,
    parent_id: Option<i64>,
    kind: &str,
    title: &str,
    slug: &str,
    description_html: &str,
    grants: &[(i64, Perms)],
) -> Result<i64, Error> {
    create_node_with_link(pool, parent_id, kind, title, slug, description_html, None, grants).await
}

/// `redirect_url` makes a link forum: it lists like a forum and leads elsewhere.
#[allow(clippy::too_many_arguments)]
pub async fn create_node_with_link(
    pool: &PgPool,
    parent_id: Option<i64>,
    kind: &str,
    title: &str,
    slug: &str,
    description_html: &str,
    redirect_url: Option<&str>,
    grants: &[(i64, Perms)],
) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO nodes (parent_id, kind, title, slug, description_html, redirect_url, position, inherit_perms)
         VALUES ($1, $2, $3, $4, $5, $6,
                 coalesce((SELECT max(position) + 1 FROM nodes WHERE parent_id IS NOT DISTINCT FROM $1), 0),
                 $7 AND $1 IS NOT NULL)
         RETURNING id",
    )
    .bind(parent_id)
    .bind(kind)
    .bind(title)
    .bind(slug)
    .bind(description_html)
    .bind(redirect_url)
    // No grants given under a parent: the new node uses the parent's.
    .bind(grants.is_empty())
    .fetch_one(&mut *tx)
    .await?;
    for (group, perms) in grants {
        sqlx::query("INSERT INTO node_permissions (node_id, group_id, perms) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(group)
            .bind(perms.0 as i32)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(id)
}

/// "Mark forum read" for one or more forums.
pub async fn mark_nodes_read(pool: &PgPool, user_id: i64, node_ids: &[i64]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO node_read_markers (user_id, node_id, marked_at) SELECT $1, unnest($2::bigint[]), now()
         ON CONFLICT (user_id, node_id) DO UPDATE SET marked_at = now()",
    )
    .bind(user_id)
    .bind(node_ids)
    .execute(&mut *tx)
    .await?;
    // Older topic markers would otherwise win over the newer forum marker. Each active topic gets
    // a position too, or the next reply would make all of it unread again.
    // These rows are not reading: `bulk` keeps them out of "topics read" (trust levels, promotion).
    // The flag tells the trigger on `read_markers` that this write is not a member reading.
    sqlx::query("SELECT set_config('bf.bulk_read', 'on', true)").execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO read_markers (user_id, item_id, last_read_post_number, bulk)
         SELECT $1, i.id, i.highest_post_number, true FROM items i
         WHERE i.node_id = ANY($2) AND i.state = 'visible'
           AND (i.last_posted_at > now() - interval '90 days'
                OR EXISTS (SELECT 1 FROM read_markers o WHERE o.user_id = $1 AND o.item_id = i.id))
         ON CONFLICT (user_id, item_id) DO UPDATE SET last_read_post_number = EXCLUDED.last_read_post_number",
    )
    .bind(user_id)
    .bind(node_ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Counts a click on a link forum and returns where it goes.
pub async fn follow_link_node(pool: &PgPool, id: i64) -> Result<Option<String>, Error> {
    sqlx::query_scalar("UPDATE nodes SET clicks = clicks + 1 WHERE id = $1 AND kind = 'link' RETURNING redirect_url")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}
