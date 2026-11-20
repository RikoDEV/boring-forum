//! The forum tree: nodes, their permissions, moderators, rules and prefixes.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::{DbResult, db_err};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::slugify;
use bf_db::admin::forums;
use serde::Deserialize;
use serde_json::json;

use super::{actor, double_option};

/// The forum structure changed: every cache that holds a copy of it goes.
fn refresh(state: &AppState) {
    state.forum.invalidate();
    state.widgets.invalidate_all();
    state.microcache.clear();
}

/// Forums that inherit their parent's permissions are about to leave it for the top level, where
/// there is nothing to inherit from: they keep the grants they were using as their own.
async fn keep_inherited(state: &AppState, ids: &[i64], actor: i64) -> Result<(), AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    for &id in ids {
        let source = bf_db::forum::nodes::grants_node(&tree.nodes, id);
        if source != id {
            forums::copy_permissions(pool, id, source, actor).await.db()?;
        }
    }
    Ok(())
}

/// The node tree with each node's grants, for the forums and permissions editor.
pub(super) async fn tree(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    let pool = state.db()?;
    let nodes = bf_db::forum::nodes::nodes(pool).await.db()?;
    let grants = bf_db::forum::nodes::node_grants(pool).await.db()?;
    let owners: std::collections::HashMap<i64, String> =
        sqlx::query_as::<_, (i64, String)>("SELECT id, plugin FROM nodes WHERE plugin IS NOT NULL")
            .fetch_all(pool)
            .await
            .db()?
            .into_iter()
            .collect();
    let out: Vec<_> = nodes
        .iter()
        .map(|n| {
            // A node that inherits shows (and is edited from) the grants it really uses.
            let from = bf_db::forum::nodes::grants_node(&nodes, n.id);
            let g: std::collections::BTreeMap<String, u32> = grants
                .get(&from.into())
                .map(|m| m.iter().map(|(gid, p)| (gid.to_string(), p.0)).collect())
                .unwrap_or_default();
            json!({
                "id": n.id, "parent_id": n.parent_id, "kind": n.kind, "title": n.title, "slug": n.slug,
                "position": n.position, "topic_count": n.topic_count, "post_count": n.post_count, "grants": g,
                "default_sort": n.default_sort, "seo_title": n.seo_title, "seo_description": n.seo_description,
                "list_style": n.list_style, "approve_mode": n.approve_mode, "min_trust_level": n.min_trust_level,
                "icon": n.icon, "icon_image": n.icon_image, "require_tag": n.require_tag, "clicks": n.clicks, "template_md": n.template_md, "counts_posts": n.counts_posts,
                "mode": n.mode, "auto_close_days": n.auto_close_days, "plugin": owners.get(&n.id),
                "inherit_perms": n.inherit_perms && n.parent_id.is_some(), "perms_from": from,
            })
        })
        .collect();
    let bits = json!({
        "view": Perms::VIEW.0, "read": Perms::READ.0, "create": Perms::CREATE.0,
        "reply": Perms::REPLY.0, "attach": Perms::ATTACH.0, "download": Perms::DOWNLOAD.0,
        "moderate": Perms::MODERATE.0,
    });
    Ok(Json(json!({ "nodes": out, "bits": bits })).into_response())
}

#[derive(Deserialize)]
pub(super) struct NodePatch {
    pub(super) title: Option<String>,
    pub(super) description: Option<String>,
    pub(super) position: Option<i32>,
    #[serde(default, deserialize_with = "double_option")]
    pub(super) parent_id: Option<Option<i64>>,
    pub(super) default_sort: Option<String>,
    pub(super) seo_title: Option<String>,
    pub(super) seo_description: Option<String>,
    pub(super) list_style: Option<String>,
    pub(super) approve_mode: Option<String>,
    pub(super) counts_posts: Option<bool>,
    pub(super) min_trust_level: Option<i16>,
    pub(super) require_tag: Option<bool>,
    pub(super) template_md: Option<String>,
    pub(super) mode: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub(super) auto_close_days: Option<Option<i32>>,
    pub(super) inherit_perms: Option<bool>,
}

#[derive(Deserialize)]
pub(super) struct NodeIcon {
    #[serde(default)]
    pub(super) icon: String,
    #[serde(default)]
    pub(super) image: String,
}

pub(super) async fn set_node_icon(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<NodeIcon>,
) -> Result<Response, AppError> {
    let icon = b.icon.trim();
    let sprite = icon.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    let emoji = !icon.chars().any(|c| c.is_ascii() || c.is_whitespace());
    if icon.chars().count() > 30 || !(sprite || emoji) {
        return Err(AppError::Validation("Pick an icon, or enter one emoji.".into()));
    }
    // Only our own uploads, so nothing else can be loaded on every forum page.
    let image = b.image.trim();
    let safe = image.is_empty()
        || (image.starts_with("/uploads/") && image.bytes().all(|c| c.is_ascii_alphanumeric() || b"/._-".contains(&c)));
    if !safe {
        return Err(AppError::Validation("The picture must be an upload.".into()));
    }
    forums::set_node_icon(state.db()?, id, icon, image).await.found()?;
    refresh(&state);
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// One forum's editable text, for the quick edit beside its name on the forum pages.
pub(super) async fn node_text(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let n = forums::node_text(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
    // Forums made before descriptions kept their Markdown: start from the text of the HTML.
    let description = if n.description_md.is_empty() && !n.description_html.is_empty() {
        bf_seo::excerpt(&n.description_html, 5000)
    } else {
        n.description_md
    };
    Ok(Json(json!({
        "title": n.title, "description": description, "mode": n.mode, "kind": n.kind,
        "icon": n.icon, "icon_image": n.icon_image,
    }))
    .into_response())
}

pub(super) async fn update_node(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(p): Json<NodePatch>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    // A plugin keeps its forum's name, text and place; permissions and the rest stay the administrator's.
    let owned: bool = sqlx::query_scalar("SELECT plugin IS NOT NULL FROM nodes WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .db()?
        .unwrap_or(false);
    if owned && (p.title.is_some() || p.description.is_some() || p.parent_id.is_some()) {
        return Err(AppError::Validation(
            "This forum belongs to a plugin, which keeps its name, text and place.".into(),
        ));
    }
    if let Some(Some(parent)) = p.parent_id
        && (parent == id || forums::creates_cycle(pool, id, parent).await.db()?)
    {
        return Err(AppError::Validation("A forum cannot be placed inside itself.".into()));
    }
    if matches!(p.parent_id, Some(None)) {
        keep_inherited(&state, &[id], actor(who)).await?;
    }
    let title = p.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let change = forums::NodeChange {
        title: title.map(str::to_owned),
        slug: title.map(slugify),
        description_html: p.description.as_deref().map(|d| bf_cook::cook(d).html),
        description_md: p.description,
        position: p.position,
        parent_id: p.parent_id,
        default_sort: p.default_sort,
        seo_title: p.seo_title,
        seo_description: p.seo_description,
        list_style: p.list_style.filter(|s| matches!(s.as_str(), "table" | "list")),
        approve_mode: p.approve_mode.filter(|s| matches!(s.as_str(), "none" | "all" | "links")),
        counts_posts: p.counts_posts,
        min_trust_level: p.min_trust_level.filter(|t| (0..=bf_core::trust::LIMIT).contains(t)),
        require_tag: p.require_tag,
        template_md: p.template_md,
        mode: p.mode.filter(|m| matches!(m.as_str(), "discussion" | "qa" | "ideas")),
        auto_close_days: p.auto_close_days.map(|d| d.filter(|d| (1..=3650).contains(d))),
        inherit_perms: p.inherit_perms,
    };
    forums::update_node(pool, id, &change, actor(who)).await.found()?;
    refresh(&state);
    crate::admin::text_overrides::refresh(&state).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub(super) struct ReorderBody {
    parent_id: Option<i64>,
    ids: Vec<i64>,
}

/// Body: `{"parent_id": 3 | null, "ids": [..]}`: the children of one parent, in the order wanted.
/// A forum listed here that was somewhere else moves under this parent.
pub(super) async fn reorder_nodes(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(body): Json<ReorderBody>,
) -> Result<Response, AppError> {
    if body.ids.len() > 2000 {
        return Err(AppError::Validation("Too many forums in one list.".into()));
    }
    let pool = state.db()?;
    if let Some(parent) = body.parent_id {
        for &id in &body.ids {
            if parent == id || forums::creates_cycle(pool, id, parent).await.db()? {
                return Err(AppError::Validation("A forum cannot be moved inside itself.".into()));
            }
        }
    }
    if body.parent_id.is_none() {
        keep_inherited(&state, &body.ids, actor(who)).await?;
    }
    forums::reorder_nodes(pool, body.parent_id, &body.ids, actor(who)).await.db()?;
    refresh(&state);
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Deletes an empty forum or category. Anything still inside it (forums, topics, a club) refuses it.
pub(super) async fn delete_node(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    use forums::NodeDelete;
    match forums::delete_node(state.db()?, id, actor(who)).await.db()? {
        NodeDelete::Deleted => {}
        NodeDelete::NotFound => return Err(AppError::NotFound),
        NodeDelete::HasChildren => {
            return Err(AppError::Validation("Move or delete the forums inside it first.".into()));
        }
        NodeDelete::HasContent => {
            return Err(AppError::Validation("It still has topics. Move them to another forum first.".into()));
        }
        NodeDelete::InClub => {
            return Err(AppError::Validation("This forum belongs to a club. Delete the club instead.".into()));
        }
    }
    refresh(&state);
    crate::admin::text_overrides::refresh(&state).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Body: `{ "group_id": bits, ... }`. Unknown bits are dropped.
pub(super) async fn set_permissions(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(body): Json<std::collections::HashMap<String, u32>>,
) -> Result<Response, AppError> {
    let grants: Vec<(i64, i32)> = body
        .into_iter()
        .map(|(g, bits)| g.parse().map(|g| (g, (bits & Perms::ALL.0) as i32)))
        .collect::<Result<_, _>>()
        .map_err(|_| AppError::Validation("Keys must be group ids.".into()))?;
    match forums::set_node_permissions(state.db()?, id, &grants, actor(who)).await {
        Ok(()) => {}
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
            return Err(AppError::Validation("Unknown forum or group.".into()));
        }
        Err(e) => return Err(db_err(e)),
    }
    refresh(&state);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn moderators(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let rows = forums::node_moderators(state.db()?, id).await.db()?;
    Ok(Json(rows.into_iter().map(|(id, username)| json!({ "id": id, "username": username })).collect::<Vec<_>>())
        .into_response())
}

#[derive(Deserialize)]
pub(super) struct ModeratorsBody {
    pub(super) usernames: Vec<String>,
}

pub(super) async fn set_moderators(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<ModeratorsBody>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let mut names: Vec<String> =
        b.usernames.iter().map(|n| n.trim().to_ascii_lowercase()).filter(|n| !n.is_empty()).collect();
    names.sort_unstable();
    names.dedup();
    let ids = bf_db::accounts::users::user_ids_by_names(pool, &names).await.db()?;
    if ids.len() != names.len() {
        return Err(AppError::Validation("Some usernames do not exist.".into()));
    }
    match forums::set_node_moderators(pool, id, &bf_core::ids::raw(&ids), actor(who)).await {
        Ok(()) => {}
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => return Err(AppError::NotFound),
        Err(e) => return Err(db_err(e)),
    }
    // Moderator scopes are part of each user's session data; it is read per request.
    state.forum.invalidate();
    state.widgets.invalidate_all();
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn set_prefixes(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
    Json(prefixes): Json<Vec<String>>,
) -> Result<Response, AppError> {
    let clean: Vec<String> = prefixes.iter().map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect();
    if clean.len() > 30 || clean.iter().any(|p| p.chars().count() > 24) {
        return Err(AppError::Validation("Up to 30 prefixes of at most 24 characters.".into()));
    }
    bf_db::admin::forums::set_forum_prefixes(state.db()?, id, &clean).await.found()?;
    refresh(&state);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub(super) struct RulesBody {
    pub(super) rules: String,
}

pub(super) async fn set_rules(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<RulesBody>,
) -> Result<Response, AppError> {
    let html = if b.rules.trim().is_empty() { String::new() } else { bf_cook::cook(&b.rules).html };
    bf_db::admin::forums::set_forum_rules(state.db()?, id, &b.rules, &html).await.found()?;
    refresh(&state);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub(super) struct CopyBody {
    pub(super) from: i64,
}

pub(super) async fn copy_permissions(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<CopyBody>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    if tree.node(id).is_none() || tree.node(b.from).is_none() {
        return Err(AppError::NotFound);
    }
    bf_db::admin::forums::copy_permissions(pool, id, b.from, actor(who)).await.db()?;
    refresh(&state);
    Ok(StatusCode::NO_CONTENT.into_response())
}
