//! Topic summaries (on request, or "since you last read") and the one-line gists on preview cards.

use super::client::{self, Chat};
use super::{DATA_RULE, check_csrf, fenced, language_name, may_send, transcript};
use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::i18n::en;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/topics/{id}/ai-summary", post(summarize).get(cached))
}

#[derive(Deserialize, Default)]
struct Ask {
    /// Only what the member has not read yet.
    #[serde(default)]
    since: bool,
}

/// A summary is fresh while fewer than this many posts came after it.
fn fresh(up_to: i32, highest: i32) -> bool {
    highest - up_to < (up_to / 10).max(3)
}

/// The summary made earlier, for anyone who can read the topic. Makes nothing and costs nothing:
/// the page asks it on load to show a summary that is already there.
async fn cached(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    if !client::configured(&state) || !state.settings().bool("ai_summaries_enabled") {
        return Err(AppError::NotFound);
    }
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    let found = bf_db::ai::summary(state.db()?, id, "summary", v.locale).await.db()?;
    match found {
        Some(s) => Ok(Json(json!({
            "summary": s.body,
            "up_to": s.up_to_post,
            "stale": !fresh(s.up_to_post, t.highest_post_number),
        }))
        .into_response()),
        None => Ok(axum::http::StatusCode::NO_CONTENT.into_response()),
    }
}

async fn summarize(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Option<Json<Ask>>,
) -> Result<Response, AppError> {
    check_csrf(&v, &headers)?;
    let s = state.settings();
    if !client::configured(&state) || !s.bool("ai_summaries_enabled") {
        return Err(AppError::NotFound);
    }
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    let pool = state.db()?;
    let locale = v.locale;
    let since = body.is_some_and(|b| b.0.since);
    if since {
        let user = v.require_user()?.id.get();
        let read = bf_db::ai::last_read(pool, id, user).await.db()?;
        if read >= t.highest_post_number {
            return Err(AppError::Validation(en("You have read everything in this topic.").into()));
        }
        let posts = bf_db::ai::topic_posts(pool, id, read, 60).await.db()?;
        if posts.is_empty() {
            return Err(AppError::Validation(en("You have read everything in this topic.").into()));
        }
        if !may_send(&state, t.node_id).await {
            return Err(super::private_forum());
        }
        let text = ask(&state, user, "summary", &t.title, &posts, locale, true).await?;
        return Ok(
            Json(json!({ "summary": text, "since": read, "posts": posts.len(), "cached": false })).into_response()
        );
    }
    if t.reply_count < s.int("ai_summary_min_replies") as i32 {
        return Err(AppError::Validation(en("This topic is too short to summarize.").into()));
    }
    if let Some(cached) = bf_db::ai::summary(pool, id, "summary", locale).await.db()?
        && fresh(cached.up_to_post, t.highest_post_number)
    {
        return Ok(Json(json!({ "summary": cached.body, "up_to": cached.up_to_post, "cached": true })).into_response());
    }
    // Making one costs money: only members, and the cached one is free for everybody.
    let user = v.require_user()?.id.get();
    if !may_send(&state, t.node_id).await {
        return Err(super::private_forum());
    }
    let posts = bf_db::ai::topic_posts(pool, id, 0, 400).await.db()?;
    let text = ask(&state, user, "summary", &t.title, &posts, locale, false).await?;
    let up_to = posts.last().map_or(0, |p| p.0);
    bf_db::ai::save_summary(pool, id, "summary", locale, up_to, &text, state.settings().str("ai_model")).await.db()?;
    Ok(Json(json!({ "summary": text, "up_to": up_to, "cached": false })).into_response())
}

async fn ask(
    state: &AppState,
    user: i64,
    feature: &'static str,
    title: &str,
    posts: &[(i32, String, String)],
    locale: &str,
    partial: bool,
) -> Result<String, AppError> {
    let lang = language_name(locale);
    let system = format!(
        "You summarize discussion threads for the people who read a forum. Write in {lang}. \
         Start with one sentence on what the thread is about{}, then 3 to 6 short bullet points (each line starting with \"- \") \
         with the main points, disagreements and any answer or decision reached. Mention who said what by @name where it matters. \
         Do not invent anything and do not add a preamble. {DATA_RULE}",
        if partial { " and what is new" } else { "" }
    );
    let prompt = format!("<posts>\nThread title: {}\n\n{}\n</posts>", fenced(title), transcript(posts, 1500, 28_000));
    let done = client::chat(state, Chat::new(feature, &system, prompt).by(user).max(700)).await?;
    Ok(done.text)
}

/// The gist of a topic for a preview card: in this language, else the site's.
pub(crate) async fn gist(state: &AppState, item: i64, locale: &str) -> Option<String> {
    if !state.settings().bool("ai_gists_enabled") {
        return None;
    }
    let pool = state.pool.as_ref()?;
    for l in [locale, bf_core::i18n::site_language(state.settings().str("default_locale"))] {
        if let Ok(Some(g)) = bf_db::ai::summary(pool, item, "gist", l).await
            && !g.body.is_empty()
        {
            return Some(g.body);
        }
    }
    None
}

/// From the tick: a few busy topics get a gist, newest activity first. Slow on purpose.
pub(crate) async fn run_gists(state: &AppState) {
    let s = state.settings();
    if !s.bool("ai_gists_enabled") {
        return;
    }
    let Some(pool) = &state.pool else { return };
    let locale = bf_core::i18n::site_language(s.str("default_locale")).to_owned();
    let Ok(work) = bf_db::ai::gist_candidates(pool, &locale, s.int("ai_summary_min_replies") as i32, 3).await else {
        return;
    };
    for (item, node, title, replies) in work {
        if !may_send(state, node).await {
            // Remember that it was looked at, or it would be first in line forever.
            let _ = bf_db::ai::save_summary(pool, item, "gist", &locale, replies, "", "").await;
            continue;
        }
        let Ok(posts) = bf_db::ai::topic_posts(pool, item, 0, 40).await else { continue };
        let system = format!(
            "Describe what a forum thread is about in ONE sentence of at most 28 words, in {}. \
             No preamble, no quotes, no markdown. {DATA_RULE}",
            language_name(&locale)
        );
        let prompt =
            format!("<posts>\nThread title: {}\n\n{}\n</posts>", fenced(&title), transcript(&posts, 600, 9_000));
        match client::chat(state, Chat::new("gist", &system, prompt).max(90)).await {
            Ok(done) => {
                let one: String =
                    client::clip(done.text.lines().next().unwrap_or_default().trim_matches(['"', ' ']), 240);
                if let Err(e) = bf_db::ai::save_summary(pool, item, "gist", &locale, replies, &one, &done.model).await {
                    tracing::warn!(error = %e, "gist could not be saved");
                }
            }
            // The provider is down or the budget is spent: try again on a later tick.
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fresh;

    #[test]
    fn a_summary_ages_with_new_posts() {
        assert!(fresh(20, 21));
        assert!(!fresh(20, 24));
        assert!(fresh(100, 105));
        assert!(!fresh(100, 110));
    }
}
