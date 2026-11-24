//! Forum administration: node settings, permissions, moderators, prefixes and rules.

use crate::admin::audit::{Entry, record};
pub use sqlx::Error;
use sqlx::PgPool;

#[derive(Debug, Default)]
pub struct NodeChange {
    pub title: Option<String>,
    pub slug: Option<String>,
    pub description_md: Option<String>,
    pub description_html: Option<String>,
    pub position: Option<i32>,
    /// `Some(None)` moves the node to the top level.
    pub parent_id: Option<Option<i64>>,
    /// Empty strings clear the three optional settings.
    pub default_sort: Option<String>,
    pub seo_title: Option<String>,
    pub seo_description: Option<String>,
    pub list_style: Option<String>,
    pub approve_mode: Option<String>,
    pub counts_posts: Option<bool>,
    pub min_trust_level: Option<i16>,
    pub require_tag: Option<bool>,
    pub template_md: Option<String>,
    pub mode: Option<String>,
    /// `Some(None)` turns inactivity closing off.
    pub auto_close_days: Option<Option<i32>>,
    /// Use the parent's permissions (`true`) or the node's own grants (`false`).
    pub inherit_perms: Option<bool>,
}

pub async fn update_node(pool: &PgPool, id: i64, c: &NodeChange, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query(
        "UPDATE nodes SET title = coalesce($2, title), slug = coalesce($3, slug),
           description_md = coalesce($4, description_md), description_html = coalesce($5, description_html),
           position = coalesce($6, position),
           parent_id = CASE WHEN $7 THEN $8 ELSE parent_id END,
           default_sort = CASE WHEN $9::text IS NULL THEN default_sort ELSE nullif($9, '') END,
           seo_title = CASE WHEN $10::text IS NULL THEN seo_title ELSE nullif($10, '') END,
           seo_description = CASE WHEN $11::text IS NULL THEN seo_description ELSE nullif($11, '') END,
           list_style = coalesce($12, list_style), approve_mode = coalesce($13, approve_mode),
           counts_posts = coalesce($14, counts_posts), min_trust_level = coalesce($15, min_trust_level),
           require_tag = coalesce($16, require_tag),
           settings = CASE WHEN $17::text IS NULL THEN settings ELSE settings || jsonb_build_object('template_md', $17::text) END,
           mode = coalesce($18, mode),
           auto_close_days = CASE WHEN $19 THEN $20 ELSE auto_close_days END,
           inherit_perms = coalesce($21, inherit_perms)
         WHERE id = $1",
    )
    .bind(id)
    .bind(&c.title)
    .bind(&c.slug)
    .bind(&c.description_md)
    .bind(&c.description_html)
    .bind(c.position)
    .bind(c.parent_id.is_some())
    .bind(c.parent_id.flatten())
    .bind(&c.default_sort)
    .bind(&c.seo_title)
    .bind(&c.seo_description)
    .bind(&c.list_style)
    .bind(&c.approve_mode)
    .bind(c.counts_posts)
    .bind(c.min_trust_level)
    .bind(c.require_tag)
    .bind(&c.template_md)
    .bind(&c.mode)
    .bind(c.auto_close_days.is_some())
    .bind(c.auto_close_days.flatten())
    .bind(c.inherit_perms)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if n == 0 {
        return Ok(false);
    }
    record(
        &mut *tx,
        Entry::new("node.update")
            .by(actor)
            .on(format!("node:{id}"))
            .details(serde_json::json!({ "title": c.title, "position": c.position, "parent_id": c.parent_id })),
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Would making `parent` the parent of `node` create a cycle?
/// What the quick edit on forum pages changes.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NodeText {
    pub title: String,
    pub description_md: String,
    pub description_html: String,
    pub mode: String,
    pub kind: String,
    pub icon: String,
    pub icon_image: String,
}

/// A forum's name, description, mode, kind and icon, for the quick edit on forum pages.
pub async fn node_text(pool: &PgPool, id: i64) -> Result<Option<NodeText>, Error> {
    sqlx::query_as(
        "SELECT title, coalesce(description_md, '') AS description_md, coalesce(description_html, '') AS description_html,
                mode, kind, coalesce(settings->>'icon', '') AS icon, coalesce(settings->>'icon_image', '') AS icon_image
         FROM nodes WHERE id = $1",
    )
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn creates_cycle(pool: &PgPool, node: i64, parent: i64) -> Result<bool, Error> {
    sqlx::query_scalar(
        "WITH RECURSIVE up AS (SELECT id, parent_id FROM nodes WHERE id = $2
                               UNION ALL SELECT n.id, n.parent_id FROM nodes n JOIN up ON n.id = up.parent_id)
         SELECT EXISTS (SELECT 1 FROM up WHERE id = $1)",
    )
    .bind(node)
    .bind(parent)
    .fetch_one(pool)
    .await
}

/// Makes `ids` the children of `parent` (`None`: the top level), in that order, first to last. A
/// forum that was inside another parent moves here. The caller checks nothing moves into itself.
pub async fn reorder_nodes(pool: &PgPool, parent: Option<i64>, ids: &[i64], actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE nodes SET parent_id = $1, position = (t.ord - 1)::int
         FROM unnest($2::bigint[]) WITH ORDINALITY AS t(id, ord)
         WHERE nodes.id = t.id",
    )
    .bind(parent)
    .bind(ids)
    .execute(&mut *tx)
    .await?;
    record(
        &mut *tx,
        Entry::new("node.reorder")
            .by(actor)
            .on(parent.map_or_else(|| "node:top".to_owned(), |p| format!("node:{p}")))
            .details(serde_json::json!(ids)),
    )
    .await?;
    tx.commit().await
}

/// What deleting a forum or category came to.
#[derive(Debug, PartialEq, Eq)]
pub enum NodeDelete {
    Deleted,
    NotFound,
    /// It still has forums inside it.
    HasChildren,
    /// It still holds topics that are not deleted.
    HasContent,
    /// A club's forum: the club goes, not the forum alone.
    InClub,
}

/// Deletes an empty forum or category with its permissions, moderators and read markers. It is
/// refused while anything is inside it, so no topic is ever lost. Topics that were already deleted
/// (waiting in the moderators' deleted items for the retention period to end) do not count: they
/// go with the forum.
pub async fn delete_node(pool: &PgPool, id: i64, actor: i64) -> Result<NodeDelete, Error> {
    let mut tx = pool.begin().await?;
    let row: Option<(String, String)> = sqlx::query_as("SELECT title, kind FROM nodes WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some((title, kind)) = row else { return Ok(NodeDelete::NotFound) };
    let has = |sql: &'static str| sqlx::query_scalar::<_, bool>(sql).bind(id);
    if has("SELECT EXISTS (SELECT 1 FROM nodes WHERE parent_id = $1)").fetch_one(&mut *tx).await? {
        return Ok(NodeDelete::HasChildren);
    }
    if has("SELECT EXISTS (SELECT 1 FROM clubs WHERE node_id = $1)").fetch_one(&mut *tx).await? {
        return Ok(NodeDelete::InClub);
    }
    if has("SELECT EXISTS (SELECT 1 FROM items WHERE node_id = $1 AND state <> 'deleted')").fetch_one(&mut *tx).await? {
        return Ok(NodeDelete::HasContent);
    }
    let purged = sqlx::query("DELETE FROM items WHERE node_id = $1").bind(id).execute(&mut *tx).await?.rows_affected();
    sqlx::query("DELETE FROM nodes WHERE id = $1").bind(id).execute(&mut *tx).await?;
    record(
        &mut *tx,
        Entry::new("node.delete")
            .by(actor)
            .on(format!("node:{id}"))
            .details(serde_json::json!({ "title": title, "kind": kind, "deleted_topics_removed": purged })),
    )
    .await?;
    tx.commit().await?;
    Ok(NodeDelete::Deleted)
}

/// Replaces a node's permission grants: (group id, bits).
pub async fn set_node_permissions(pool: &PgPool, node: i64, grants: &[(i64, i32)], actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM node_permissions WHERE node_id = $1").bind(node).execute(&mut *tx).await?;
    // Setting grants by hand means the node uses them, not its parent's.
    sqlx::query("UPDATE nodes SET inherit_perms = false WHERE id = $1").bind(node).execute(&mut *tx).await?;
    let (groups, bits): (Vec<i64>, Vec<i32>) = grants.iter().copied().unzip();
    sqlx::query(
        "INSERT INTO node_permissions (node_id, group_id, perms) SELECT $1, g, p FROM unnest($2::bigint[], $3::int[]) AS t(g, p) WHERE p <> 0",
    )
    .bind(node)
    .bind(&groups)
    .bind(&bits)
    .execute(&mut *tx)
    .await?;
    record(
        &mut *tx,
        Entry::new("node.permissions").by(actor).on(format!("node:{node}")).details(serde_json::json!(grants)),
    )
    .await?;
    tx.commit().await
}

pub async fn node_moderators(pool: &PgPool, node: i64) -> Result<Vec<(i64, String)>, Error> {
    sqlx::query_as(
        "SELECT u.id, u.username FROM node_moderators m JOIN users u ON u.id = m.user_id WHERE m.node_id = $1 ORDER BY u.username",
    )
    .bind(node)
    .fetch_all(pool)
    .await
}

/// Replaces a forum's scoped moderators (node moderators).
pub async fn set_node_moderators(pool: &PgPool, node: i64, user_ids: &[i64], actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM node_moderators WHERE node_id = $1").bind(node).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO node_moderators (node_id, user_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING",
    )
    .bind(node)
    .bind(user_ids)
    .execute(&mut *tx)
    .await?;
    record(
        &mut *tx,
        Entry::new("node.moderators").by(actor).on(format!("node:{node}")).details(serde_json::json!(user_ids)),
    )
    .await?;
    tx.commit().await
}

/// A category or forum's icon: a sprite name or emoji, and/or an uploaded picture (which wins).
pub async fn set_node_icon(pool: &PgPool, id: i64, icon: &str, image: &str) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE nodes SET settings = settings || jsonb_build_object('icon', nullif($2, ''), 'icon_image', nullif($3, '')) WHERE id = $1")
        .bind(id)
        .bind(icon)
        .bind(image)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Allowed prefixes for a forum, from `nodes.settings->'prefixes'`.
pub async fn forum_prefixes(pool: &PgPool, node: i64) -> Result<Vec<String>, Error> {
    let v: Option<serde_json::Value> = sqlx::query_scalar("SELECT settings->'prefixes' FROM nodes WHERE id = $1")
        .bind(node)
        .fetch_optional(pool)
        .await?
        .flatten();
    Ok(v.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())
}

pub async fn set_forum_prefixes(pool: &PgPool, node: i64, prefixes: &[String]) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE nodes SET settings = jsonb_set(settings, '{prefixes}', $2) WHERE id = $1")
        .bind(node)
        .bind(serde_json::json!(prefixes))
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Forum rules, shown above the topic list: (markdown, cooked html).
pub async fn forum_rules(pool: &PgPool, node: i64) -> Result<(Option<String>, Option<String>), Error> {
    sqlx::query_as("SELECT settings->>'rules_md', settings->>'rules_html' FROM nodes WHERE id = $1")
        .bind(node)
        .fetch_one(pool)
        .await
}

pub async fn set_forum_rules(pool: &PgPool, node: i64, md: &str, html: &str) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE nodes SET settings = settings || jsonb_build_object('rules_md', $2::text, 'rules_html', $3::text) WHERE id = $1")
        .bind(node)
        .bind(md)
        .bind(html)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Replaces a forum's permission grants with another forum's.
pub async fn copy_permissions(pool: &PgPool, to: i64, from: i64, actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM node_permissions WHERE node_id = $1").bind(to).execute(&mut *tx).await?;
    sqlx::query("UPDATE nodes SET inherit_perms = false WHERE id = $1").bind(to).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO node_permissions (node_id, group_id, perms) SELECT $1, group_id, perms FROM node_permissions WHERE node_id = $2")
        .bind(to)
        .bind(from)
        .execute(&mut *tx)
        .await?;
    record(
        &mut *tx,
        Entry::new("node.permissions_copy")
            .by(actor)
            .on(format!("node:{to}"))
            .details(serde_json::json!({ "from": from })),
    )
    .await?;
    tx.commit().await
}
