//! Local actors and objects as ActivityPub sees them: URIs, which posts may leave the site, `Note` documents and who an activity is addressed to.

use crate::forum::tree::guest;
use crate::state::AppState;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::federation;
use serde_json::{Value, json};

use super::*;

pub(super) fn actor_uri(state: &AppState, owner_type: &str, id: i64) -> String {
    match owner_type {
        "user" => format!("{}/ap/u/{id}", base(state)),
        "forum" => format!("{}/ap/c/{id}", base(state)),
        _ => format!("{}/ap/actor", base(state)),
    }
}

pub(super) fn note_uri(state: &AppState, post: i64) -> String {
    format!("{}/ap/p/{post}", base(state))
}

/// Forums whose content may leave the site: readable by guests.
pub(super) async fn public_forum(state: &AppState, node: i64) -> bool {
    let Some(pool) = &state.pool else { return false };
    let Ok(tree) = state.forum.get(pool).await else { return false };
    tree.node(node).is_some_and(|n| n.kind == "forum") && tree.perms(&guest(), node).has(Perms::READ)
}

/// A post that may be shown to the fediverse: visible, in a visible topic in a public forum,
/// by a local member.
pub(super) async fn federable_post(state: &AppState, id: i64) -> Option<(federation::ApPost, federation::LocalUser)> {
    let pool = state.pool.as_ref()?;
    let p = federation::post(pool, id).await.ok()??;
    if p.state != "visible" || p.topic_state != "visible" || !public_forum(state, p.node_id).await {
        return None;
    }
    let author = federation::local_user(pool, p.author_id?).await.ok()??;
    Some((p, author))
}

pub(super) fn absolute(state: &AppState, html: &str) -> String {
    let b = base(state);
    html.replace("src=\"/", &format!("src=\"{b}/")).replace("href=\"/", &format!("href=\"{b}/"))
}

pub(super) async fn reply_target(state: &AppState, p: &federation::ApPost) -> Option<String> {
    let pool = state.pool.as_ref()?;
    let target = match p.reply_to_post_id {
        Some(r) => Some(r),
        None if p.post_number > 1 => p.first_post_id,
        None => None,
    }?;
    // A reply to a post that came from elsewhere points at the original object.
    if let Ok(Some((uri, _))) = federation::object_of_post(pool, target).await {
        return Some(uri);
    }
    Some(note_uri(state, target))
}

pub(super) async fn note_json(state: &AppState, p: &federation::ApPost, author: &federation::LocalUser) -> Value {
    let b = base(state);
    let me = actor_uri(state, "user", author.id);
    let forum = actor_uri(state, "forum", p.node_id);
    let mut content = absolute(state, &p.cooked);
    if p.post_number == 1 {
        content = format!("<p><strong>{}</strong></p>{content}", bf_core::html::esc(&p.title));
    }
    let url = if p.post_number == 1 {
        format!("{b}{}", topic_path(p.item_id, &p.slug))
    } else {
        format!("{b}{}#post-{}", topic_path(p.item_id, &p.slug), p.post_number)
    };
    let mut cc = vec![json!(format!("{me}/followers")), json!(forum)];
    let in_reply_to = reply_target(state, p).await;
    if let (Some(target), Some(pool)) = (&in_reply_to, &state.pool)
        && let Ok(Some((_, actor))) = federation::object(pool, target).await
        && let Ok(Some(a)) = sqlx::query_scalar::<_, String>("SELECT uri FROM ap_actors WHERE id = $1")
            .bind(actor)
            .fetch_optional(pool)
            .await
    {
        cc.push(json!(a));
    }
    let mut note = json!({
        "id": note_uri(state, p.id),
        "type": "Note",
        "attributedTo": me,
        "to": [AS_PUBLIC],
        "cc": cc,
        "audience": forum,
        "context": format!("{b}{}", topic_path(p.item_id, &p.slug)),
        "content": content,
        "mediaType": "text/html",
        "url": url,
        "published": p.created_at.to_rfc3339(),
        "inReplyTo": in_reply_to,
        "sensitive": false,
    });
    if p.post_number == 1 {
        note["name"] = json!(p.title);
    }
    if p.edit_count > 0 {
        note["updated"] = json!(p.updated_at.to_rfc3339());
    }
    if p.post_number == 1 {
        extended::decorate_note(state, p.item_id, &mut note).await;
    }
    note
}

/// Where an activity about a post goes: the author's followers, the forum's followers, and the
/// author of a remote post being answered.
pub(super) async fn audience(state: &AppState, p: &federation::ApPost, author: i64) -> Vec<String> {
    let Some(pool) = &state.pool else { return vec![] };
    let mut inboxes = federation::follower_inboxes(pool, "user", author).await.unwrap_or_default();
    inboxes.extend(federation::follower_inboxes(pool, "forum", p.node_id).await.unwrap_or_default());
    if let Some(target) = reply_target(state, p).await
        && let Ok(Some((_, actor))) = federation::object(pool, &target).await
        && let Ok(Some(inbox)) =
            sqlx::query_scalar::<_, String>("SELECT coalesce(shared_inbox, inbox) FROM ap_actors WHERE id = $1")
                .bind(actor)
                .fetch_optional(pool)
                .await
    {
        inboxes.push(inbox);
    }
    inboxes
}
