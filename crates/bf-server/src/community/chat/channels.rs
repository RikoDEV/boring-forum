//! Chat pages, the channel list, direct messages and reading a channel's messages.

use crate::http::render::{DbResult, layout, page};
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_db::community::chat;
use bf_web::pages::{ChatPage, render};
use serde::Deserialize;
use serde_json::json;

use super::{enabled, viewable};

pub(super) async fn chat_page(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    show(&state, &v, 0)
}

pub(super) async fn chat_channel_page(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    viewable(&state, &v, id).await?;
    show(&state, &v, id)
}

pub(super) fn show(state: &AppState, v: &Viewer, channel: i64) -> Result<Response, AppError> {
    enabled(state)?;
    let user = v.require_user()?;
    let (mut l, set) = layout(state, v, Some(&v.t().msg("Chat")));
    l.noindex = true;
    let t = v.t();
    let tip = |setting: &str| crate::community::trust::needs_text(state, &t, v, setting);
    let props = json!({
        "editorUrl": l.editor_url, "channel": channel, "me": user.username, "staff": is_staff(v),
        "needs": {
            "write": tip("chat_min_trust_level"),
            "dm": tip("chat_dm_min_trust_level"),
            "create": tip("chat_create_min_trust_level"),
        },
    })
    .to_string();
    let body = ChatPage { layout: l, props };
    Ok(page(render(&body), set, false))
}

pub(super) async fn list_channels(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    enabled(&state)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    Ok(Json(
        chat::channels_for(pool, user.id.get(), &bf_core::ids::raw(&v.actor.groups), v.actor.is_admin)
            .await
            .db()?,
    )
    .into_response())
}

#[derive(Deserialize)]
pub(super) struct DmBody {
    pub(super) usernames: Vec<String>,
}

pub(super) async fn open_dm(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<DmBody>,
) -> Result<Response, AppError> {
    enabled(&state)?;
    v.check_csrf_headers(&headers)?;
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    if !is_staff(&v) && i64::from(user.trust_level) < state.settings().int("chat_dm_min_trust_level") {
        return Err(crate::community::trust::level_refusal(
            &state,
            &v.t(),
            user.trust_level,
            state.settings().int("chat_dm_min_trust_level"),
        ));
    }
    let pool = state.db()?;
    let names: Vec<String> =
        b.usernames.iter().map(|n| n.trim().trim_start_matches('@').to_lowercase()).filter(|n| !n.is_empty()).collect();
    let max = state.settings().int("chat_max_dm_users") as usize;
    if names.is_empty() || names.len() > max {
        return Err(AppError::invalid("Choose 1 to {max} people.", &[("max", &max)]));
    }
    let mut people = bf_core::ids::raw(&bf_db::accounts::users::user_ids_by_names(pool, &names).await.db()?);
    people.retain(|p| *p != user.id);
    if people.is_empty() {
        return Err(AppError::Validation("There is no member by that name.".into()));
    }
    // Ignoring the sender, or taking messages from nobody or only from people they follow: the
    // "who can message me" setting covers chat too.
    if chat::ignoring(pool, &people, user.id.get()).await.db()?
        || !bf_db::community::messages::restricting(pool, user.id.get(), &people).await.db()?.is_empty()
    {
        return Err(AppError::Validation("You cannot message that member.".into()));
    }
    people.push(user.id.get());
    let id = chat::open_dm(pool, &people, user.id.get()).await.db()?;
    Ok(Json(json!({ "id": id })).into_response())
}

#[derive(Deserialize)]
pub(super) struct PageQuery {
    pub(super) before: Option<i64>,
    #[serde(default)]
    pub(super) q: String,
    #[serde(default)]
    pub(super) pinned: bool,
}

pub(super) async fn messages(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Query(q): Query<PageQuery>,
) -> Result<Response, AppError> {
    viewable(&state, &v, id).await?;
    // Searching a channel reads all of it: bounded in length and in how often a member may ask.
    let term: String = q.q.trim().chars().take(100).collect();
    let term = term.as_str();
    if !term.is_empty() {
        let key = v.user.as_ref().map_or_else(|| format!("ip:{}", crate::http::ratelimit::ip_key(v.ip)), |u| format!("u:{}", u.id));
        crate::http::ratelimit::SEARCH_BY_USER.enforce(&key)?;
    }
    let term = if term.is_empty() {
        String::new()
    } else {
        crate::platform::plugins::filter_text(&state, "search.query", "q", term).await?
    };
    let term = Some(term.as_str()).filter(|s| !s.is_empty());
    let rows = chat::messages(state.db()?, id, q.before, term, q.pinned).await.db()?;
    Ok(Json(rows).into_response())
}

/// What the viewer's chat page needs to know about them.
pub(super) async fn me(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    enabled(&state)?;
    let user = v.require_user()?;
    let ignored = chat::ignored_names(state.db()?, user.id.get()).await.db()?;
    let s = state.settings();
    let staff = crate::http::viewer::is_staff(&v);
    let types = crate::forum::uploads::allowed_types(&state, !staff);
    let need = s.int("chat_upload_min_trust_level").max(s.int("upload_min_trust_level"));
    // What the attach button needs: whether it shows at all, the kinds it offers, how many per message.
    let uploads = s.bool("chat_allow_uploads") && !types.is_empty() && (staff || i64::from(user.trust_level) >= need);
    Ok(Json(json!({
        "id": user.id,
        "ignored": ignored,
        "uploads": uploads,
        "upload_types": types,
        "max_files": if staff { 20 } else { s.int("chat_max_files_per_message") },
    }))
    .into_response())
}
