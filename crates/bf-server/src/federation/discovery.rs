//! WebFinger, host-meta and NodeInfo.

use crate::http::render::DbResult;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::slug::{forum_path, profile_path};
use bf_db::federation;
use serde::Deserialize;
use serde_json::json;

use super::*;

#[derive(Deserialize)]
pub(super) struct WebfingerQuery {
    resource: String,
}

pub(super) async fn webfinger(
    State(state): State<AppState>,
    Query(q): Query<WebfingerQuery>,
) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let pool = state.db()?;
    let b = base(&state);
    let h = host(&state);
    let r = q.resource.trim();
    // acct:name@host, or the actor or profile URL itself.
    let name = if let Some(acct) = r.strip_prefix("acct:") {
        let (name, at) = acct.rsplit_once('@').ok_or(AppError::NotFound)?;
        if !at.eq_ignore_ascii_case(&h) {
            return Err(AppError::NotFound);
        }
        name.to_owned()
    } else if let Some(rest) = r.strip_prefix(&format!("{b}/u/")) {
        rest.to_owned()
    } else if let Some(id) = r.strip_prefix(&format!("{b}/ap/u/")).and_then(|i| i.parse::<i64>().ok()) {
        federation::local_user(pool, id).await.db()?.ok_or(AppError::NotFound)?.username
    } else {
        return Err(AppError::NotFound);
    };
    let (actor, profile, subject) = if let Some(slug) = name.strip_prefix("forum.") {
        let tree = state.forum.get(pool).await?;
        let node = tree
            .nodes
            .iter()
            .find(|n| n.slug.eq_ignore_ascii_case(slug) && n.kind == "forum")
            .ok_or(AppError::NotFound)?;
        if !public_forum(&state, node.id).await {
            return Err(AppError::NotFound);
        }
        (
            actor_uri(&state, "forum", node.id),
            format!("{b}{}", forum_path(node.id, &node.slug)),
            format!("acct:forum.{}@{h}", node.slug),
        )
    } else {
        let u = federation::local_user_by_name(pool, &name).await.db()?.ok_or(AppError::NotFound)?;
        (
            actor_uri(&state, "user", u.id),
            format!("{b}{}", profile_path(u.id, &u.username)),
            format!("acct:{}@{h}", u.username),
        )
    };
    let mut res = Json(json!({
        "subject": subject,
        "aliases": [actor, profile],
        "links": [
            { "rel": "self", "type": AP_TYPE, "href": actor },
            { "rel": "http://webfinger.net/rel/profile-page", "type": "text/html", "href": profile },
        ],
    }))
    .into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/jrd+json; charset=utf-8"));
    res.headers_mut().insert("access-control-allow-origin", HeaderValue::from_static("*"));
    Ok(res)
}

pub(super) async fn host_meta(State(state): State<AppState>) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><XRD xmlns="http://docs.oasis-open.org/ns/xri/xrd-1.0"><Link rel="lrdd" template="{}/.well-known/webfinger?resource={{uri}}"/></XRD>"#,
        base(&state)
    );
    Ok(([(header::CONTENT_TYPE, "application/xrd+xml; charset=utf-8")], body).into_response())
}

pub(super) async fn nodeinfo_links(State(state): State<AppState>) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "links": [{ "rel": "http://nodeinfo.diaspora.software/ns/schema/2.1", "href": format!("{}/nodeinfo/2.1", base(&state)) }] }))
        .into_response())
}

pub(super) async fn nodeinfo(State(state): State<AppState>) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let (total, month, half, posts) = federation::usage(state.db()?).await.db()?;
    let s = state.settings();
    let mut res = Json(json!({
        "version": "2.1",
        "software": { "name": "boring-forum", "version": env!("CARGO_PKG_VERSION"), "repository": "https://github.com/RikoDEV/boring-forum" },
        "protocols": ["activitypub"],
        "services": { "inbound": [], "outbound": ["atom1.0", "rss2.0"] },
        "openRegistrations": s.bool("allow_registration") && !s.bool("invite_only"),
        "usage": { "users": { "total": total, "activeMonth": month, "activeHalfyear": half }, "localPosts": posts },
        "metadata": { "nodeName": s.str("site_name"), "nodeDescription": s.str("site_description") },
    }))
    .into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; profile=\"http://nodeinfo.diaspora.software/ns/schema/2.1#\""),
    );
    Ok(res)
}
