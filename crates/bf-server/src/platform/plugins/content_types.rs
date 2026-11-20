//! Content types a plugin registers: entries of a plugin's own tables that take part in
//! the site's search, wait in a moderator's queue before they are published, and are gated by one
//! of the plugin's permissions. The core keeps the index and the queue; the plugin keeps the entry.

use crate::http::i18n::{tr, trh};
use crate::http::render::{DbResult, pool};
use crate::http::ui::{Shell, begin, finish, panel};
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::i18n::en;
use bf_db::platform::plugin_content as pc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::host::{local_path, storage_check};
use super::ops::failed;
use super::stats::allow;
use super::{call, listening};

const MAX_TYPES: usize = 10;

/// `[[content_types]]`: a kind of entry the plugin keeps.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct ContentTypeDecl {
    pub(super) key: String,
    /// English name, shown in search results and the review queue.
    #[serde(default)]
    pub(super) label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) label_key: Option<String>,
    /// Entries may be offered to the site's search (`search.index`).
    #[serde(default)]
    pub(super) search: bool,
    /// Entries may be held for a moderator (`review.hold`).
    #[serde(default)]
    pub(super) review: bool,
    /// A key of the plugin's `[[permissions]]`: search shows an entry only to members whose group
    /// holds it, and `content.can` answers for it.
    #[serde(default)]
    pub(super) permission: Option<String>,
}

pub(super) fn validate(types: &[ContentTypeDecl], capabilities: &[String], permissions: &[String]) -> Result<(), String> {
    if types.is_empty() {
        return Ok(());
    }
    if !capabilities.iter().any(|c| c == "content_types") {
        return Err(en("A plugin that declares content types must ask for the content_types capability.").into());
    }
    if types.len() > MAX_TYPES {
        return Err(format!("A plugin may declare {MAX_TYPES} content types at most."));
    }
    for (i, t) in types.iter().enumerate() {
        if !super::registry::good_key(&t.key) || types[..i].iter().any(|o| o.key == t.key) {
            return Err(format!("Bad or repeated content type {}.", t.key));
        }
        if t.label.trim().is_empty() && t.label_key.is_none() {
            return Err(format!("Content type {} has no label.", t.key));
        }
        if let Some(p) = &t.permission
            && !permissions.contains(p)
        {
            return Err(format!("Content type {} needs permission {p}, which the plugin does not declare.", t.key));
        }
    }
    Ok(())
}

fn declared<'a>(types: &'a [ContentTypeDecl], req: &Value) -> Result<&'a ContentTypeDecl, String> {
    let key = req.get("type").and_then(Value::as_str).unwrap_or_default();
    types.iter().find(|t| t.key == key).ok_or_else(|| en("This plugin declares no such content type.").to_owned())
}

fn short<'a>(req: &'a Value, field: &str, max: usize) -> Result<&'a str, String> {
    let s = req.get(field).and_then(Value::as_str).unwrap_or_default();
    if s.is_empty() || s.chars().count() > max {
        return Err(format!("{field}: 1 to {max} characters."));
    }
    Ok(s)
}

/// The operations of the `content_types` capability.
pub(super) async fn more(
    state: &AppState,
    plugin: &str,
    types: &[ContentTypeDecl],
    op: &str,
    req: &Value,
) -> Option<Result<Value, String>> {
    Some(match op {
        "search.index" => search_index(state, plugin, types, req).await,
        "search.remove" => search_remove(state, plugin, types, req).await,
        "review.hold" => review_hold(state, plugin, types, req).await,
        "review.withdraw" => review_withdraw(state, plugin, types, req).await,
        "content.can" => content_can(state, plugin, types, req).await,
        _ => return None,
    })
}

/// `search.index`: `{type, ref, title, body, url, author?}`. The entry's permission comes from the
/// declaration, never from the request.
async fn search_index(state: &AppState, plugin: &str, types: &[ContentTypeDecl], req: &Value) -> Result<Value, String> {
    let t = declared(types, req)?;
    if !t.search {
        return Err(en("This content type is not searchable.").into());
    }
    let (reference, title, url) = (short(req, "ref", 100)?, short(req, "title", 200)?, short(req, "url", 300)?);
    let body: String = req.get("body").and_then(Value::as_str).unwrap_or_default().chars().take(60_000).collect();
    if !local_path(url) {
        return Err(en("The url must be a path on this site.").into());
    }
    if !allow(plugin, "search_index", 60, 600, 1) {
        return Err(en("This plugin has indexed too much in the last minute.").into());
    }
    storage_check(state, plugin).await?;
    let author = req.get("author").and_then(Value::as_str).map(|a| a.chars().take(60).collect::<String>());
    let doc = pc::SearchDoc {
        plugin,
        kind: &t.key,
        reference,
        title,
        body: &body,
        url,
        author: author.as_deref(),
        perm: t.permission.as_deref(),
    };
    let pool = pool(state).map_err(failed)?;
    if !pc::index_doc(pool, &doc).await.map_err(failed)? {
        return Err(en("This plugin keeps too many documents in search.").into());
    }
    Ok(json!({ "indexed": true }))
}

async fn search_remove(state: &AppState, plugin: &str, types: &[ContentTypeDecl], req: &Value) -> Result<Value, String> {
    let t = declared(types, req)?;
    let reference = short(req, "ref", 100)?;
    let removed = pc::remove_doc(pool(state).map_err(failed)?, plugin, &t.key, reference).await.map_err(failed)?;
    Ok(json!({ "removed": removed }))
}

/// `review.hold`: `{type, ref, title, excerpt?, url, user_id?}`. A moderator sees it at
/// `/review/apps`; the decision comes back as the `content.review` hook.
async fn review_hold(state: &AppState, plugin: &str, types: &[ContentTypeDecl], req: &Value) -> Result<Value, String> {
    let t = declared(types, req)?;
    if !t.review {
        return Err(en("This content type is not held for review.").into());
    }
    let (reference, title, url) = (short(req, "ref", 100)?, short(req, "title", 200)?, short(req, "url", 300)?);
    if !local_path(url) {
        return Err(en("The url must be a path on this site.").into());
    }
    if !allow(plugin, "review_hold", 3600, 120, 1) {
        return Err(en("This plugin has held too many entries in the last hour.").into());
    }
    storage_check(state, plugin).await?;
    let excerpt: String = req.get("excerpt").and_then(Value::as_str).unwrap_or_default().chars().take(500).collect();
    let by = req.get("user_id").and_then(Value::as_i64);
    let held = pc::Held { plugin, kind: &t.key, reference, title, excerpt: &excerpt, url, by };
    let id = pc::hold(pool(state).map_err(failed)?, &held).await.map_err(failed)?;
    Ok(json!({ "held": id.is_some(), "id": id }))
}

async fn review_withdraw(state: &AppState, plugin: &str, types: &[ContentTypeDecl], req: &Value) -> Result<Value, String> {
    let t = declared(types, req)?;
    let reference = short(req, "ref", 100)?;
    let gone = pc::withdraw(pool(state).map_err(failed)?, plugin, &t.key, reference).await.map_err(failed)?;
    Ok(json!({ "withdrawn": gone }))
}

/// `content.can`: `{user_id, type}` answers `{allowed}`: whether the member holds the permission
/// of the content type (always true for a type with none). For jobs and hooks that run with no viewer.
async fn content_can(state: &AppState, plugin: &str, types: &[ContentTypeDecl], req: &Value) -> Result<Value, String> {
    let t = declared(types, req)?;
    let user = req.get("user_id").and_then(Value::as_i64).unwrap_or_default();
    let Some(perm) = &t.permission else { return Ok(json!({ "allowed": true })) };
    let allowed = bf_db::platform::registry_groups::holds_permission(pool(state).map_err(failed)?, plugin, perm, user)
        .await
        .map_err(failed)?;
    Ok(json!({ "allowed": allowed }))
}

/// Entries waiting for a decision (moderators and administrators), for the badge on the queue.
pub(crate) async fn pending_count(state: &AppState) -> i64 {
    let Some(pool) = state.pool.as_ref() else { return 0 };
    sqlx::query_scalar(
        "SELECT count(*) FROM plugin_review_items r JOIN plugins p ON p.id = r.plugin AND p.enabled WHERE r.status = 'pending'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0)
}

/// `/review/apps`: what plugins hold for review. Staff only; anyone else gets the 404 page.
pub(super) async fn review_page(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    v.require_user().map_err(|_| AppError::NotFound)?;
    if !is_staff(&v) {
        return Err(AppError::NotFound);
    }
    let rows = pc::pending(state.db()?, 200).await.db()?;
    let ctx = begin(&state, &v);
    let mut html = String::new();
    if rows.is_empty() {
        html.push_str(&format!("<p>{}</p>", trh("Nothing from plugins is waiting.")));
    }
    for r in &rows {
        let by = r.submitted_by.as_deref().map(|n| format!(" - {}", esc(n))).unwrap_or_default();
        html.push_str(&format!(
            "<article class=\"bf-box\"><h2><a href=\"{url}\">{title}</a></h2><p class=\"small\">{plugin} / {kind}{by}</p><p>{excerpt}</p>\
             <form method=\"post\" action=\"/review/apps/{id}\" class=\"bf-modal-actions\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
             <button class=\"btn\" name=\"decision\" value=\"approve\">{approve}</button>\
             <button class=\"btn btn-danger\" name=\"decision\" value=\"reject\">{reject}</button></form></article>",
            url = esc(&r.url),
            title = esc(&r.title),
            plugin = esc(&r.plugin),
            kind = esc(&r.kind),
            excerpt = esc(&r.excerpt),
            id = r.id,
            csrf = esc(&ctx.token),
            approve = trh("Approve"),
            reject = trh("Reject"),
        ));
    }
    let shell = Shell { title: tr("Held by plugins"), noindex: true, ..Shell::default() };
    Ok(finish(&state, &v, ctx, shell, panel(&html)))
}

#[derive(Deserialize)]
pub(super) struct Decision {
    csrf: String,
    decision: String,
}

/// Approve or reject one held entry, then tell the plugin (`content.review`) so it can publish or drop it.
pub(super) async fn decide(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    axum::Form(f): axum::Form<Decision>,
) -> Result<Response, AppError> {
    let user = v.require_user().map_err(|_| AppError::NotFound)?;
    if !is_staff(&v) {
        return Err(AppError::NotFound);
    }
    v.check_csrf(&f.csrf)?;
    let approve = match f.decision.as_str() {
        "approve" => true,
        "reject" => false,
        _ => return Err(AppError::BadRequest("Choose approve or reject.".into())),
    };
    let done = pc::decide(state.db()?, id, approve, user.id.get()).await.db()?;
    if let Some((plugin, kind, reference)) = done
        && let Some(p) = listening("content.review").into_iter().find(|p| p.id == plugin)
    {
        let data = json!({
            "type": kind, "ref": reference, "decision": if approve { "approve" } else { "reject" },
            "by": user.id.get(),
        });
        let _ = call(&state, p, "content.review", &data).await;
        state.microcache.clear();
    }
    Ok(Redirect::to("/review/apps").into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(key: &str) -> ContentTypeDecl {
        ContentTypeDecl { key: key.into(), label: "Entry".into(), label_key: None, search: true, review: true, permission: None }
    }

    #[test]
    fn types_need_the_capability_and_a_declared_permission() {
        let caps = vec!["content_types".to_owned()];
        assert!(validate(&[decl("entry")], &caps, &[]).is_ok());
        assert!(validate(&[decl("entry")], &[], &[]).is_err());
        assert!(validate(&[decl("entry"), decl("entry")], &caps, &[]).is_err());
        let mut gated = decl("entry");
        gated.permission = Some("write".into());
        assert!(validate(std::slice::from_ref(&gated), &caps, &[]).is_err());
        assert!(validate(&[gated], &caps, &["write".to_owned()]).is_ok());
    }

    #[test]
    fn only_declared_types_can_be_used() {
        let mut quiet = decl("note");
        quiet.search = false;
        assert!(declared(&[quiet.clone()], &json!({ "type": "note" })).is_ok());
        assert!(declared(&[quiet], &json!({ "type": "other" })).is_err());
    }
}
