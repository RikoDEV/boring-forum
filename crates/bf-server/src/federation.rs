//! ActivityPub: two-way federation of the core flows.
//!
//! Actors: members (`Person`), forums readable by guests (`Group`, announcing new topics and
//! posts per FEP-1b12), and the instance (`Application`, which signs fetches). Discovery through
//! WebFinger and NodeInfo 2.1. Outgoing Create/Update/Delete for posts in guest-readable forums,
//! Like/Undo for reactions on remote posts, Accept for follows. Incoming Follow/Undo, replies
//! (Create of a Note whose `inReplyTo` is a post here), Update and Delete by the same author,
//! Like/Undo as reactions, and Delete of an actor.
//!
//! Requests are signed with draft-cavage HTTP Signatures (RSA-SHA256 over `(request-target)`,
//! `host`, `date`, `digest`), the scheme every major implementation accepts. Incoming
//! activities must be signed by the key of their `actor`, carry a matching SHA-256 `Digest`,
//! and be dated within five minutes. Every outbound request goes through the webhook SSRF guard.
//!
//! `extended` adds forums following remote groups, account moves, polls as `Question`s and
//! direct messages (`federation_dms`).
//!
//! Off unless `federation_enabled` is set, and off while `login_required` is on (a site that
//! shows nothing to guests must not hand its members and posts to other servers): then every
//! endpoint here answers 404.

use crate::http::render::DbResult;
use crate::state::AppState;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::federation;
use rsa::sha2::Digest as _;
use serde_json::{Value, json};

mod actors;
mod discovery;
mod events;
pub(crate) mod extended;
mod inbox;
mod manage;
mod objects;
mod outbound;
mod signatures;
mod text;

use self::actors::{
    dm_object, forum_actor, forum_followers, forum_outbox, instance_actor, note, user_actor, user_followers,
    user_outbox,
};
use self::discovery::{host_meta, nodeinfo, nodeinfo_links, webfinger};
use self::inbox::{create_reply, inbox, local_post_for, local_target};
use self::manage::{admin_add_mirror, admin_mirrors, admin_remove_mirror, fediverse_page, move_account, save_aliases};
use self::objects::{absolute, actor_uri, audience, federable_post, note_json, note_uri, public_forum, reply_target};
use self::outbound::{blocked, domain_of, enqueue, fetch, remote_actor, str_of};
use self::signatures::{key_for, parse_signature, public_key_pem, sign, signing_string, verify_with};
use self::text::remote_text;

pub use self::actors::negotiate;
pub(crate) use self::events::{actor_deleted, on_event, post_deleted, reaction_changed};
pub use self::outbound::spawn_worker;
pub(crate) use self::text::strip_html;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/webfinger", get(webfinger))
        .route("/.well-known/nodeinfo", get(nodeinfo_links))
        .route("/.well-known/host-meta", get(host_meta))
        .route("/nodeinfo/2.1", get(nodeinfo))
        .route("/ap/actor", get(instance_actor))
        .route("/ap/inbox", post(inbox))
        .route("/ap/u/{id}", get(user_actor))
        .route("/ap/u/{id}/inbox", post(inbox))
        .route("/ap/u/{id}/outbox", get(user_outbox))
        .route("/ap/u/{id}/followers", get(user_followers))
        .route("/ap/c/{id}", get(forum_actor))
        .route("/ap/c/{id}/inbox", post(inbox))
        .route("/ap/c/{id}/outbox", get(forum_outbox))
        .route("/ap/c/{id}/followers", get(forum_followers))
        .route("/ap/p/{id}", get(note))
        .route("/ap/m/{id}", get(dm_object))
        .route(
            "/api/v1/admin/federation/mirrors",
            get(admin_mirrors).post(admin_add_mirror).delete(admin_remove_mirror),
        )
        .route("/settings/fediverse", get(fediverse_page))
        .route("/settings/fediverse/aliases", post(save_aliases))
        .route("/settings/fediverse/move", post(move_account))
}

const AS_PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";

const AP_TYPE: &str = "application/activity+json";

const MAX_INBOX: usize = 256 * 1024;

const MAX_FETCH: usize = 512 * 1024;

const MAX_ATTEMPTS: i32 = 8;

fn context() -> Value {
    json!(["https://www.w3.org/ns/activitystreams", "https://w3id.org/security/v1"])
}

pub fn enabled(state: &AppState) -> bool {
    let s = state.settings();
    s.bool("federation_enabled") && !s.bool("login_required")
}

fn base(state: &AppState) -> String {
    state.config.base_url.trim_end_matches('/').to_owned()
}

fn host(state: &AppState) -> String {
    url::Url::parse(&state.config.base_url)
        .ok()
        .and_then(|u| {
            u.host_str().map(|h| match u.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_owned(),
            })
        })
        .unwrap_or_default()
}

fn activity_json(v: Value) -> Response {
    let mut res = Json(v).into_response();
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/activity+json; charset=utf-8"));
    res.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept"));
    res
}
