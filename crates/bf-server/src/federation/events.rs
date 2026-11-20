//! Site events turned into outgoing activities.

use crate::state::AppState;
use bf_db::federation;
use serde_json::{Value, json};

use super::*;

/// Called for every site event (`platform::emit`); sends what the fediverse should hear.
pub(crate) async fn on_event(state: &AppState, event: &str, payload: &Value) {
    if !enabled(state) || !matches!(event, "topic.created" | "post.created" | "post.edited") {
        return;
    }
    let Some(pool) = &state.pool else { return };
    let (Some(topic), Some(number)) = (payload["topic_id"].as_i64(), payload["post_number"].as_i64()) else { return };
    let Ok(Some(post_id)) = federation::post_id_at(pool, topic, number as i32).await else { return };
    let Some((p, author)) = federable_post(state, post_id).await else { return };
    let note = note_json(state, &p, &author).await;
    let me = actor_uri(state, "user", author.id);
    let inboxes = audience(state, &p, author.id).await;
    if event == "post.edited" {
        let activity = json!({
            "@context": context(), "id": format!("{}/update/{}", note["id"].as_str().unwrap_or_default(), p.edit_count),
            "type": "Update", "actor": me, "to": note["to"], "cc": note["cc"], "object": note,
        });
        enqueue(state, &inboxes, ("user", author.id), &activity).await;
        return;
    }
    let create = json!({
        "@context": context(), "id": format!("{}/activity", note["id"].as_str().unwrap_or_default()),
        "type": "Create", "actor": me, "published": note["published"], "to": note["to"], "cc": note["cc"], "object": note,
    });
    let author_followers = federation::follower_inboxes(pool, "user", author.id).await.unwrap_or_default();
    let mut direct = author_followers;
    if let Some(target) = reply_target(state, &p).await
        && let Ok(Some((_, actor))) = federation::object(pool, &target).await
        && let Ok(Some(inbox)) =
            sqlx::query_scalar::<_, String>("SELECT coalesce(shared_inbox, inbox) FROM ap_actors WHERE id = $1")
                .bind(actor)
                .fetch_optional(pool)
                .await
    {
        direct.push(inbox);
    }
    enqueue(state, &direct, ("user", author.id), &create).await;
    // FEP-1b12: the forum announces what is posted in it to its own followers.
    let forum = actor_uri(state, "forum", p.node_id);
    let forum_inboxes = federation::follower_inboxes(pool, "forum", p.node_id).await.unwrap_or_default();
    if !forum_inboxes.is_empty() {
        let announce = json!({
            "@context": context(), "id": format!("{forum}/announce/{}", p.id), "type": "Announce", "actor": forum,
            "to": [AS_PUBLIC], "cc": [format!("{forum}/followers")], "object": note["id"], "published": note["published"],
        });
        enqueue(state, &forum_inboxes, ("forum", p.node_id), &announce).await;
    }
}

/// A post was removed here: its remote copies are deleted too.
pub(crate) async fn post_deleted(state: &AppState, post_id: i64, author: Option<i64>) {
    if !enabled(state) {
        return;
    }
    let (Some(pool), Some(author)) = (&state.pool, author) else { return };
    if federation::local_user(pool, author).await.ok().flatten().is_none() {
        return;
    }
    let Ok(Some(p)) = federation::post(pool, post_id).await else { return };
    if !public_forum(state, p.node_id).await {
        return;
    }
    let me = actor_uri(state, "user", author);
    let uri = note_uri(state, post_id);
    let activity = json!({
        "@context": context(), "id": format!("{uri}/delete"), "type": "Delete", "actor": me, "to": [AS_PUBLIC],
        "object": { "id": uri, "type": "Tombstone" },
    });
    let inboxes = audience(state, &p, author).await;
    enqueue(state, &inboxes, ("user", author), &activity).await;
}

/// A member's account is going away (deleted or merged): their followers' servers are told the
/// actor is deleted. Called before the account is anonymized; the key that signs the delivery is
/// removed by housekeeping once the queue has drained.
pub(crate) async fn actor_deleted(state: &AppState, user: i64) {
    if !enabled(state) {
        return;
    }
    let Some(pool) = &state.pool else { return };
    let inboxes = federation::follower_inboxes(pool, "user", user).await.unwrap_or_default();
    if inboxes.is_empty() {
        return;
    }
    let me = actor_uri(state, "user", user);
    let activity = json!({
        "@context": context(), "id": format!("{me}#delete"), "type": "Delete", "actor": me, "to": [AS_PUBLIC],
        "object": me,
    });
    enqueue(state, &inboxes, ("user", user), &activity).await;
}

/// A member liked or unliked a post. Only remote posts need a message: the author is elsewhere.
pub(crate) async fn reaction_changed(state: &AppState, post_id: i64, user_id: i64, liked: bool) {
    if !enabled(state) {
        return;
    }
    let Some(pool) = &state.pool else { return };
    let Ok(Some((object, actor))) = federation::object_of_post(pool, post_id).await else { return };
    if federation::local_user(pool, user_id).await.ok().flatten().is_none() {
        return;
    }
    let Ok(Some(inbox)) = sqlx::query_scalar::<_, String>("SELECT inbox FROM ap_actors WHERE id = $1")
        .bind(actor)
        .fetch_optional(pool)
        .await
    else {
        return;
    };
    let me = actor_uri(state, "user", user_id);
    let like_id = format!("{me}/like/{post_id}");
    let like = json!({ "id": like_id, "type": "Like", "actor": me, "object": object });
    let activity = if liked {
        json!({ "@context": context(), "id": like_id, "type": "Like", "actor": me, "object": object })
    } else {
        json!({ "@context": context(), "id": format!("{like_id}/undo/{}", chrono::Utc::now().timestamp()), "type": "Undo", "actor": me, "object": like })
    };
    enqueue(state, &[inbox], ("user", user_id), &activity).await;
}
