//! Actor, collection and note documents, and content negotiation that sends fediverse lookups of profile, forum and topic URLs to them.

use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, Method, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_core::slug::{forum_path, profile_path};
use bf_db::federation;
use serde_json::{Value, json};

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn actor_doc(
    state: &AppState,
    kind: &str,
    uri: &str,
    username: &str,
    name: &str,
    summary: &str,
    url: &str,
    pem: &str,
) -> Value {
    json!({
        "@context": context(),
        "id": uri,
        "type": kind,
        "preferredUsername": username,
        "name": name,
        "summary": summary,
        "url": url,
        "inbox": format!("{uri}/inbox"),
        "outbox": format!("{uri}/outbox"),
        "followers": format!("{uri}/followers"),
        "endpoints": { "sharedInbox": format!("{}/ap/inbox", base(state)) },
        "manuallyApprovesFollowers": false,
        "discoverable": true,
        "publicKey": { "id": format!("{uri}#main-key"), "owner": uri, "publicKeyPem": pem },
    })
}

pub(super) async fn instance_actor(State(state): State<AppState>) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let pem = public_key_pem(&state, "instance", 0).await?;
    let uri = actor_uri(&state, "instance", 0);
    let s = state.settings();
    let mut doc = actor_doc(
        &state,
        "Application",
        &uri,
        &host(&state),
        s.str("site_name"),
        s.str("site_description"),
        &base(&state),
        &pem,
    );
    doc["inbox"] = json!(format!("{}/ap/inbox", base(&state)));
    Ok(activity_json(doc))
}

pub(super) async fn user_actor(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let u = federation::local_user(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
    let pem = public_key_pem(&state, "user", id).await?;
    let uri = actor_uri(&state, "user", id);
    let b = base(&state);
    let mut doc = actor_doc(
        &state,
        "Person",
        &uri,
        &u.username,
        u.display_name.as_deref().unwrap_or(&u.username),
        &absolute(&state, u.about_html.as_deref().unwrap_or_default()),
        &format!("{b}{}", profile_path(u.id, &u.username)),
        &pem,
    );
    doc["icon"] =
        json!({ "type": "Image", "mediaType": "image/svg+xml", "url": format!("{b}/avatar/{}.svg", u.username) });
    doc["published"] = json!(u.joined_at.to_rfc3339());
    extended::decorate_actor(&state, id, &mut doc).await;
    Ok(activity_json(doc))
}

pub(super) async fn forum_actor(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Response, AppError> {
    if !enabled(&state) || !public_forum(&state, id).await {
        return Err(AppError::NotFound);
    }
    let tree = state.forum.get(state.db()?).await?;
    let n = tree.node(id).ok_or(AppError::NotFound)?;
    let pem = public_key_pem(&state, "forum", id).await?;
    let uri = actor_uri(&state, "forum", id);
    let mut doc = actor_doc(
        &state,
        "Group",
        &uri,
        &format!("forum.{}", n.slug),
        &n.title,
        &absolute(&state, &n.description_html),
        &format!("{}{}", base(&state), forum_path(n.id, &n.slug)),
        &pem,
    );
    // FEP-1b12: only members of this site post here; remote replies arrive as replies.
    doc["postingRestrictedToMods"] = json!(true);
    Ok(activity_json(doc))
}

pub(super) fn collection(uri: &str, total: i64) -> Response {
    activity_json(
        json!({ "@context": "https://www.w3.org/ns/activitystreams", "id": uri, "type": "OrderedCollection", "totalItems": total }),
    )
}

pub(super) async fn user_outbox(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    federation::local_user(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
    Ok(collection(&format!("{}/outbox", actor_uri(&state, "user", id)), 0))
}

pub(super) async fn user_followers(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let pool = state.db()?;
    federation::local_user(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let n = federation::follower_count(pool, "user", id).await.db()?;
    Ok(collection(&format!("{}/followers", actor_uri(&state, "user", id)), n))
}

pub(super) async fn forum_outbox(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Response, AppError> {
    if !enabled(&state) || !public_forum(&state, id).await {
        return Err(AppError::NotFound);
    }
    Ok(collection(&format!("{}/outbox", actor_uri(&state, "forum", id)), 0))
}

pub(super) async fn forum_followers(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Response, AppError> {
    if !enabled(&state) || !public_forum(&state, id).await {
        return Err(AppError::NotFound);
    }
    let n = federation::follower_count(state.db()?, "forum", id).await.db()?;
    Ok(collection(&format!("{}/followers", actor_uri(&state, "forum", id)), n))
}

pub(super) async fn note(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let (p, author) = federable_post(&state, id).await.ok_or(AppError::NotFound)?;
    let mut doc = note_json(&state, &p, &author).await;
    doc["@context"] = context();
    Ok(activity_json(doc))
}

/// Content negotiation: a fediverse server looking up a profile, forum or topic URL with an
/// ActivityPub `Accept` is sent to the matching actor or object.
pub async fn negotiate(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let wants_ap = req.method() == Method::GET
        && req.headers().get(header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|a| {
            a.contains("application/activity+json")
                || (a.contains("application/ld+json") && a.contains("activitystreams"))
        });
    if !wants_ap || !enabled(&state) {
        return next.run(req).await;
    }
    let path = req.uri().path().to_owned();
    let Some(pool) = state.pool.clone() else { return next.run(req).await };
    let b = base(&state);
    let target = if let Some(name) = path.strip_prefix("/u/") {
        let name = percent_decode(name.trim_end_matches('/'));
        federation::local_user_by_name(&pool, &name).await.ok().flatten().map(|u| actor_uri(&state, "user", u.id))
    } else if let Some(rest) = path.strip_prefix("/t/") {
        let id = rest.rsplit('/').next().and_then(|i| i.parse::<i64>().ok());
        match id {
            Some(topic) => federation::post_id_at(&pool, topic, 1).await.ok().flatten().map(|p| note_uri(&state, p)),
            None => None,
        }
    } else if let Some(rest) = path.strip_prefix("/c/") {
        rest.rsplit('/').next().and_then(|i| i.parse::<i64>().ok()).map(|id| actor_uri(&state, "forum", id))
    } else {
        None
    };
    match target {
        Some(t) if t.starts_with(&b) => Redirect::to(&t).into_response(),
        _ => next.run(req).await,
    }
}

pub(super) fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={s}").as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default()
}

/// A direct message's `Note`, served only to the people it was addressed to.
pub(super) async fn dm_object(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    extended::fetch_dm(&state, id, &uri, &headers).await
}
