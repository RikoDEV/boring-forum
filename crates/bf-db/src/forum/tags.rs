//! Tags, tag groups and muted tags.

use crate::forum::topics::TOPIC_COLS;
use crate::forum::topics::TopicRow;
use crate::forum::topics::topic_joins;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub topics: i64,
    /// The plugin that declared this tag: it is managed there.
    pub plugin: Option<String>,
}

/// (item id, [(name, slug)]) for a page of topics, in one query.
pub async fn tags_of(pool: &PgPool, items: &[i64]) -> Result<Vec<(i64, String, String)>, Error> {
    sqlx::query_as(
        "SELECT it.item_id, t.name, t.slug FROM item_tags it JOIN tags t ON t.id = it.tag_id
         WHERE it.item_id = ANY($1) ORDER BY t.name",
    )
    .bind(items)
    .fetch_all(pool)
    .await
}

/// Replaces a topic's tags. Unknown tags are created only when `create` is set.
pub async fn set_tags(pool: &PgPool, item: i64, tags: &[(String, String)], create: bool) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM item_tags WHERE item_id = $1").bind(item).execute(&mut *tx).await?;
    for (name, slug) in tags {
        if create {
            sqlx::query("INSERT INTO tags (name, slug) VALUES ($1, $2) ON CONFLICT (slug) DO NOTHING")
                .bind(name)
                .bind(slug)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO item_tags (item_id, tag_id) SELECT $1, id FROM tags WHERE slug = $2 ON CONFLICT DO NOTHING",
        )
        .bind(item)
        .bind(slug)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// A tag without any topic yet. `None` when one with that address already exists.
pub async fn create_tag(pool: &PgPool, name: &str, slug: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("INSERT INTO tags (name, slug) VALUES ($1, $2) ON CONFLICT (slug) DO NOTHING RETURNING id")
        .bind(name)
        .bind(slug)
        .fetch_optional(pool)
        .await
}

pub async fn all_tags(pool: &PgPool) -> Result<Vec<Tag>, Error> {
    sqlx::query_as(
        "SELECT t.id, t.name, t.slug, count(i.id) AS topics, t.plugin FROM tags t
         LEFT JOIN item_tags it ON it.tag_id = t.id
         LEFT JOIN items i ON i.id = it.item_id AND i.state = 'visible' AND i.listed
         GROUP BY t.id ORDER BY t.name LIMIT 2000",
    )
    .fetch_all(pool)
    .await
}

/// Tags that start with `prefix` and are used by at least one topic in `nodes` (the forums the
/// viewer can read): a tag's name can give away what a private forum talks about, also when its
/// last topic is gone, so a tag nobody can see used is not listed.
pub async fn tags_starting(
    pool: &PgPool,
    prefix: &str,
    limit: i64,
    nodes: &[i64],
) -> Result<Vec<(String, String)>, Error> {
    let escaped = prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    sqlx::query_as(
        "SELECT t.name, t.slug FROM tags t WHERE t.name ILIKE $1 || '%'
           AND EXISTS (SELECT 1 FROM item_tags x JOIN items xi ON xi.id = x.item_id
                       WHERE x.tag_id = t.id AND xi.node_id = ANY($3))
         ORDER BY t.name LIMIT $2",
    )
    .bind(escaped)
    .bind(limit)
    .bind(nodes)
    .fetch_all(pool)
    .await
}

/// A tag's name, if a topic in `nodes` uses it (see [`tags_starting`]).
pub async fn tag_by_slug(pool: &PgPool, slug: &str, nodes: &[i64]) -> Result<Option<String>, Error> {
    sqlx::query_scalar(
        "SELECT t.name FROM tags t WHERE t.slug = $1
           AND EXISTS (SELECT 1 FROM item_tags x JOIN items xi ON xi.id = x.item_id
                       WHERE x.tag_id = t.id AND xi.node_id = ANY($2))",
    )
    .bind(slug)
    .bind(nodes)
    .fetch_optional(pool)
    .await
}

/// [`all_tags`] as one viewer may see it: only tags used by a visible, listed topic in `nodes`, and
/// the topic count covers those topics only.
pub async fn all_tags_in(pool: &PgPool, nodes: &[i64]) -> Result<Vec<Tag>, Error> {
    sqlx::query_as(
        "SELECT t.id, t.name, t.slug, count(it.item_id) FILTER (WHERE i.node_id = ANY($1)) AS topics, t.plugin
         FROM tags t LEFT JOIN item_tags it ON it.tag_id = t.id
         LEFT JOIN items i ON i.id = it.item_id AND i.state = 'visible' AND i.listed
         GROUP BY t.id
         HAVING count(it.item_id) FILTER (WHERE i.node_id = ANY($1)) > 0
         ORDER BY t.name LIMIT 2000",
    )
    .bind(nodes)
    .fetch_all(pool)
    .await
}

pub async fn rename_tag(pool: &PgPool, id: i64, name: &str, slug: &str) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE tags SET name = $2, slug = $3 WHERE id = $1 AND plugin IS NULL")
        .bind(id)
        .bind(name)
        .bind(slug)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

pub async fn delete_tag(pool: &PgPool, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM tags WHERE id = $1 AND plugin IS NULL").bind(id).execute(pool).await?.rows_affected()
        > 0)
}

pub async fn tag_topics(
    pool: &PgPool,
    slug: &str,
    nodes: &[i64],
    viewer: Option<i64>,
    limit: i64,
) -> Result<Vec<TopicRow>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TOPIC_COLS} {} WHERE i.node_id = ANY($1) AND i.state = 'visible' AND i.listed
           AND EXISTS (SELECT 1 FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE it.item_id = i.id AND t.slug = $2)
         ORDER BY i.last_posted_at DESC, i.id DESC LIMIT $4",
        topic_joins(3)
    )))
    .bind(nodes)
    .bind(slug)
    .bind(viewer)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn muted_tag_names(pool: &PgPool, user: i64) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(
        "SELECT t.name FROM muted_tags m JOIN tags t ON t.id = m.tag_id WHERE m.user_id = $1 ORDER BY t.name",
    )
    .bind(user)
    .fetch_all(pool)
    .await
}

pub async fn set_muted_tags(pool: &PgPool, user: i64, slugs: &[String]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM muted_tags WHERE user_id = $1").bind(user).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO muted_tags (user_id, tag_id) SELECT $1, id FROM tags WHERE slug = ANY($2)")
        .bind(user)
        .bind(slugs)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagGroup {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    /// A topic may carry only one tag from this group.
    #[serde(default)]
    pub one_per_topic: bool,
    /// Tag slugs in the group.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Forums where the group's tags may be used; empty means everywhere.
    #[serde(default)]
    pub nodes: Vec<i64>,
}

pub async fn tag_groups(pool: &PgPool) -> Result<Vec<TagGroup>, Error> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(i64, String, bool, Vec<String>, Vec<i64>)> = sqlx::query_as(
        "SELECT g.id, g.name, g.one_per_topic,
                coalesce((SELECT array_agg(t.slug ORDER BY t.slug) FROM tag_group_members m JOIN tags t ON t.id = m.tag_id
                          WHERE m.group_id = g.id), '{}'),
                coalesce((SELECT array_agg(n.node_id ORDER BY n.node_id) FROM tag_group_nodes n WHERE n.group_id = g.id), '{}')
         FROM tag_groups g ORDER BY g.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, one_per_topic, tags, nodes)| TagGroup { id, name, one_per_topic, tags, nodes })
        .collect())
}

/// Creates (`id` 0) or replaces a group. Returns its id, or `None` if that id does not exist.
pub async fn save_tag_group(pool: &PgPool, id: i64, g: &TagGroup) -> Result<Option<i64>, Error> {
    let mut tx = pool.begin().await?;
    let id: Option<i64> = if id == 0 {
        sqlx::query_scalar("INSERT INTO tag_groups (name, one_per_topic) VALUES ($1, $2) RETURNING id")
            .bind(&g.name)
            .bind(g.one_per_topic)
            .fetch_optional(&mut *tx)
            .await?
    } else {
        sqlx::query_scalar("UPDATE tag_groups SET name = $2, one_per_topic = $3 WHERE id = $1 RETURNING id")
            .bind(id)
            .bind(&g.name)
            .bind(g.one_per_topic)
            .fetch_optional(&mut *tx)
            .await?
    };
    let Some(id) = id else { return Ok(None) };
    sqlx::query("DELETE FROM tag_group_members WHERE group_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM tag_group_nodes WHERE group_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO tag_group_members (group_id, tag_id) SELECT $1, id FROM tags WHERE slug = ANY($2)")
        .bind(id)
        .bind(&g.tags)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO tag_group_nodes (group_id, node_id) SELECT $1, n.id FROM nodes n WHERE n.id = ANY($2)")
        .bind(id)
        .bind(&g.nodes)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(id))
}

crate::sql::delete_by_id!(delete_tag_group, "tag_groups");

/// The first reason these tags cannot go on this topic, if any: a tag restricted to other
/// forums, or more than one tag from a one-per-topic group.
pub async fn tag_rule_problem(pool: &PgPool, item: i64, slugs: &[String]) -> Result<Option<String>, Error> {
    let node: i64 = sqlx::query_scalar("SELECT node_id FROM items WHERE id = $1").bind(item).fetch_one(pool).await?;
    tag_rule_problem_in_node(pool, node, slugs).await
}

/// The same rules before a topic exists, given the forum it is going into.
pub async fn tag_rule_problem_in_node(pool: &PgPool, node: i64, slugs: &[String]) -> Result<Option<String>, Error> {
    let restricted: Option<String> = sqlx::query_scalar(
        "SELECT t.name FROM tags t
         JOIN tag_group_members m ON m.tag_id = t.id
         WHERE t.slug = ANY($2)
           AND EXISTS (SELECT 1 FROM tag_group_nodes n WHERE n.group_id = m.group_id)
           AND NOT EXISTS (SELECT 1 FROM tag_group_nodes n WHERE n.group_id = m.group_id AND n.node_id = $1)
         LIMIT 1",
    )
    .bind(node)
    .bind(slugs)
    .fetch_optional(pool)
    .await?;
    if let Some(name) = restricted {
        return Ok(Some(format!("The tag {name} cannot be used in this forum.")));
    }
    let crowded: Option<String> = sqlx::query_scalar(
        "SELECT g.name FROM tag_groups g JOIN tag_group_members m ON m.group_id = g.id JOIN tags t ON t.id = m.tag_id
         WHERE g.one_per_topic AND t.slug = ANY($1) GROUP BY g.id, g.name HAVING count(*) > 1 LIMIT 1",
    )
    .bind(slugs)
    .fetch_optional(pool)
    .await?;
    Ok(crowded.map(|name| format!("Choose only one tag from {name}.")))
}
