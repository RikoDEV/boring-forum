//! The composer helper: proofread, change the tone, shorten, explain, and suggest titles, tags
//! and a forum for a draft. It works on the member's own draft, on request, and changes nothing
//! by itself: the answer goes back to the editor, which shows it for the member to accept.

use super::client::{self, Chat, clip, json_in};
use super::{DATA_RULE, check_csrf, fenced, language_name, may_send};
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_core::perms::Perms;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/ai/helper", post(helper))
}

#[derive(Deserialize)]
struct Ask {
    action: String,
    text: String,
    /// Language code for `translate`.
    #[serde(default)]
    to: String,
    /// The forum the draft is for, when known.
    #[serde(default)]
    forum: Option<i64>,
    /// The topic it replies to, when it is a reply.
    #[serde(default)]
    topic: Option<i64>,
}

const REWRITES: [&str; 6] = ["proofread", "friendly", "formal", "shorten", "explain", "translate"];

fn instruction(action: &str, lang: &str, to: &str) -> String {
    let rule = "Keep Markdown, BBCode, links, @mentions, emoji, code blocks and line breaks as they are. \
                Answer with ONLY the resulting text, no preamble and no quotes.";
    match action {
        "proofread" => format!(
            "Fix spelling, grammar and punctuation. Do not change the meaning, wording choices or voice. Keep the language of the text. {rule}"
        ),
        "friendly" => format!(
            "Rewrite in a warmer, friendlier tone without changing the meaning. Keep the language of the text. {rule}"
        ),
        "formal" => format!(
            "Rewrite in a more formal, professional tone without changing the meaning. Keep the language of the text. {rule}"
        ),
        "shorten" => {
            format!("Make it shorter and clearer, keeping every important point. Keep the language of the text. {rule}")
        }
        "explain" => format!(
            "Explain what the text means in simple words, in {lang}, in at most 120 words. Answer with ONLY the explanation."
        ),
        _ => format!("Translate into {to}. If it is already in {to}, return it unchanged. {rule}"),
    }
}

async fn helper(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(ask): Json<Ask>,
) -> Result<Response, AppError> {
    check_csrf(&v, &headers)?;
    let user = v.require_user()?;
    let s = state.settings();
    if !client::configured(&state) || !s.bool("ai_helper_enabled") {
        return Err(AppError::NotFound);
    }
    if i64::from(user.trust_level) < s.int("ai_helper_min_trust_level") && !v.actor.is_admin {
        return Err(AppError::Forbidden);
    }
    let text = ask.text.trim();
    if text.is_empty() || text.chars().count() > 8000 {
        return Err(AppError::Validation(en("Write something first, up to 8000 characters.").into()));
    }
    // A draft for a private forum stays off the provider unless the administrator allowed it.
    let mut node = ask.forum;
    if let Some(t) = ask.topic {
        node = sqlx::query_scalar("SELECT node_id FROM items WHERE id = $1")
            .bind(t)
            .fetch_optional(state.db()?)
            .await
            .db()?;
    }
    if let Some(f) = node
        && !may_send(&state, f).await
    {
        return Err(super::private_forum());
    }
    let uid = user.id.get();
    let lang = language_name(v.locale);
    let pool = state.db()?;
    let data = format!("<posts>\n{}\n</posts>", fenced(&bf_cook::strip_hidden(text)));
    match ask.action.as_str() {
        a if REWRITES.contains(&a) => {
            let to = language_name(if bf_core::i18n::supported(&ask.to) { &ask.to } else { v.locale });
            let system =
                format!("You help a forum member improve their own draft. {} {DATA_RULE}", instruction(a, &lang, &to));
            let done = client::chat(&state, Chat::new("helper", &system, data).by(uid).max(2000)).await?;
            Ok(Json(json!({ "text": done.text })).into_response())
        }
        "titles" => {
            let system = format!(
                "Suggest 3 clear, specific titles (at most 80 characters, no quotes, no trailing period) for a forum topic that begins with the text. \
                 Write them in the language of the text. Answer with ONLY a JSON object {{\"titles\": [\"…\", \"…\", \"…\"]}}. {DATA_RULE}"
            );
            let done = client::chat(&state, Chat::new("helper", &system, data).by(uid).max(200)).await?;
            let titles: Vec<String> = json_in(&done.text)
                .and_then(|j| j["titles"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|t| t.as_str())
                .map(|t| clip(t.trim(), 100))
                .filter(|t| !t.is_empty())
                .take(3)
                .collect();
            Ok(Json(json!({ "titles": titles })).into_response())
        }
        "tags" => {
            // Only tags used in forums this member can read: a private forum's tags must not reach the model.
            let readable = state.forum.get(pool).await?.readable(&v.actor);
            let mut known = bf_db::forum::tags::all_tags_in(pool, &readable).await.db()?;
            known.sort_by_key(|t| std::cmp::Reverse(t.topics));
            let names: Vec<String> = known.iter().take(80).map(|t| t.name.clone()).collect();
            let system = format!(
                "Suggest up to 5 short tags for a forum topic that begins with the text. Prefer these existing tags when they fit: {}. \
                 You may add a new one if nothing fits. Answer with ONLY a JSON object {{\"tags\": [\"…\"]}}. {DATA_RULE}",
                names.join(", ")
            );
            let done = client::chat(&state, Chat::new("helper", &system, data).by(uid).max(120)).await?;
            let tags: Vec<String> = json_in(&done.text)
                .and_then(|j| j["tags"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|t| t.as_str())
                .map(|t| clip(t.trim().trim_start_matches('#'), 40))
                .filter(|t| !t.is_empty())
                .take(5)
                .collect();
            Ok(Json(json!({ "tags": tags })).into_response())
        }
        "forum" => {
            let tree = state.forum.get(pool).await?;
            let choices: Vec<(i64, String)> = tree
                .nodes
                .iter()
                .filter(|n| n.kind == "forum" && tree.perms(&v.actor, n.id).has(Perms::CREATE))
                .map(|n| (n.id, n.title.clone()))
                .collect();
            if choices.is_empty() {
                return Ok(Json(json!({ "forum": null })).into_response());
            }
            let list: Vec<String> = choices.iter().map(|(id, t)| format!("{id}: {t}")).collect();
            let system = format!(
                "Pick the one forum where a topic that begins with the text belongs. The forums, as id: name:\n{}\n\
                 Answer with ONLY a JSON object {{\"forum\": <id>}}. {DATA_RULE}",
                list.join("\n")
            );
            let done = client::chat(&state, Chat::new("helper", &system, data).by(uid).max(30)).await?;
            let pick = json_in(&done.text).and_then(|j| j["forum"].as_i64());
            let found = pick.and_then(|id| choices.iter().find(|c| c.0 == id));
            Ok(Json(json!({ "forum": found.map(|(id, title)| json!({ "id": id, "title": title })) })).into_response())
        }
        _ => Err(AppError::BadRequest("unknown action".into())),
    }
}
