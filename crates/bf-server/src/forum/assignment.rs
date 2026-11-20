//! Topic assignment.

use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::ui::simple_page;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::forum::assignment;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/t/{id}/assign", post(assign)).route("/assigned", get(assigned))
}

#[derive(Deserialize)]
struct AssignForm {
    csrf: String,
    #[serde(default)]
    username: String,
}

/// Moderators assign a topic to a member who moderates it; an empty name clears it.
async fn assign(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<AssignForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, tree, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    let pool = state.db()?;
    let name = f.username.trim().to_lowercase();
    let target = if name.is_empty() {
        None
    } else {
        let refused = || AppError::Validation("Only a moderator of this forum can be assigned.".into());
        let ids = bf_db::accounts::users::user_ids_by_names(pool, &[name]).await.db()?;
        let who = ids.first().copied().ok_or_else(refused)?;
        let u = bf_db::accounts::users::user_by_id(pool, who).await.db()?.ok_or_else(refused)?;
        let actor = bf_core::perms::Actor {
            user_id: Some(u.id),
            groups: u.groups.clone(),
            is_admin: u.is_admin,
            silenced: u.silenced,
            moderated_nodes: u.moderated_nodes.clone(),
            ..Default::default()
        };
        if !tree.perms(&actor, t.node_id).has(Perms::MODERATE) {
            return Err(refused());
        }
        Some(who)
    };
    assignment::set_assignee(pool, id, target.map(bf_core::ids::UserId::get), user.id.get()).await.db()?;
    if let Some(who) = target.filter(|w| *w != user.id) {
        let data = json!({ "by": user.username, "title": t.title, "url": topic_path(t.id, &t.slug) });
        crate::community::notifications::send(&state, user.id.get(), &[who.get()], "assigned", &data).await;
    }
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}

async fn assigned(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let nodes: Vec<i64> =
        tree.nodes.iter().filter(|n| tree.perms(&v.actor, n.id).has(Perms::MODERATE)).map(|n| n.id).collect();
    if nodes.is_empty() {
        return Err(AppError::Forbidden);
    }
    let rows = assignment::assigned_topics(pool, user.id.get(), &nodes).await.db()?;
    let items: String = rows
        .iter()
        .map(|(id, slug, title)| format!("<li><a href=\"{}\">{}</a></li>", esc(&topic_path(*id, slug)), esc(title)))
        .collect();
    Ok(simple_page(&state, &v, &v.t().msg("Assigned to me"), &format!("<ul class=\"activity\">{items}</ul>"), false))
}
