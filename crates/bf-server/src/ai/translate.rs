//! Machine translation of posts: when a reader asks for one, and for new topics in the languages
//! an administrator listed. Results are stored with the other translations (marked as made by
//! machine), so the next reader in that language gets it without another request.

use super::client::{self, Chat, json_in};
use super::{DATA_RULE, actor, check_csrf, fenced, language_name, may_send, moderation::NewPost};
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
use bf_db::forum::{posts, translations};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/posts/{id}/ai-translate", post(translate_post))
}

#[derive(Deserialize, Default)]
struct Ask {
    /// The language code; the reader's own when left out.
    #[serde(default)]
    to: String,
}

/// The title (first posts only) and body in `locale`.
async fn translate(
    state: &AppState,
    user: Option<i64>,
    title: Option<&str>,
    raw: &str,
    locale: &str,
) -> Result<(String, String), AppError> {
    let lang = language_name(locale);
    let system = format!(
        "You translate forum posts into {lang}. Keep the meaning, tone and formatting exactly: Markdown, BBCode, code blocks, \
         links, @mentions, emoji and line breaks stay as they are, and code is never translated. If the text is already in {lang}, \
         return it unchanged. Answer with ONLY a JSON object {{\"title\": \"…\", \"text\": \"…\"}} (\"title\" is empty when no title is given). {DATA_RULE}"
    );
    let prompt = format!(
        "<posts>\nTitle: {}\n\nText:\n{}\n</posts>",
        fenced(title.unwrap_or_default()),
        fenced(&client::clip(&bf_cook::strip_hidden(raw), 12_000))
    );
    let mut chat = Chat::new("translate", &system, prompt).max(4000);
    if let Some(u) = user {
        chat = chat.by(u);
    }
    let done = client::chat(state, chat).await?;
    let v = json_in(&done.text)
        .ok_or_else(|| AppError::Validation(en("The AI service is not available right now.").into()))?;
    let text = v["text"].as_str().unwrap_or_default().trim().to_owned();
    if text.is_empty() {
        return Err(AppError::Validation(en("The AI service is not available right now.").into()));
    }
    Ok((v["title"].as_str().unwrap_or_default().trim().chars().take(200).collect(), text))
}

async fn save(state: &AppState, post: i64, locale: &str, title: &str, raw: &str) -> Result<String, AppError> {
    let pool = state.db()?;
    let bot = actor(state).await.ok_or_else(|| AppError::internal("no AI account"))?;
    // What the model wrote is shown as the post: the site's watched words apply to it as they
    // did to the original, and a translation with a blocked word in it is not kept.
    let refused = || AppError::Validation(en("The AI service is not available right now.").into());
    let (raw, _) = super::watched(state, raw).await.ok_or_else(refused)?;
    let (title, _) = super::watched(state, title).await.ok_or_else(refused)?;
    let (raw, title) = (raw.as_str(), title.as_str());
    let cooked = crate::platform::plugins::cook(state, raw).await;
    translations::save_machine(pool, post, locale, title, raw, &cooked.html, bot).await.db()?;
    crate::forum::translations::note_post_translation();
    state.microcache.clear();
    Ok(cooked.html)
}

async fn translate_post(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Option<Json<Ask>>,
) -> Result<Response, AppError> {
    check_csrf(&v, &headers)?;
    let s = state.settings();
    if !client::configured(&state) || !s.bool("ai_translate_enabled") || !s.bool("content_translation_enabled") {
        return Err(AppError::NotFound);
    }
    let user = v.require_user()?.id.get();
    let pool = state.db()?;
    let post = posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (_, _, perms) = readable_topic(&state, &v, post.item_id).await?;
    if post.state != "visible" && !perms.has(bf_core::perms::Perms::MODERATE) {
        return Err(AppError::NotFound);
    }
    let to = body.map(|b| b.0.to).filter(|t| !t.is_empty()).unwrap_or_else(|| v.locale.to_owned());
    if !bf_core::i18n::supported(&to) {
        return Err(AppError::Validation(en("That language is not supported.").into()));
    }
    // A machine translation older than the post's last edit is not served: it is made again.
    if let Some((_, _, cooked, machine)) = translations::one_current(pool, id, &to).await.db()? {
        return Ok(Json(json!({ "cooked": cooked, "machine": machine, "cached": true })).into_response());
    }
    if !may_send(&state, post.node_id).await {
        return Err(super::private_forum());
    }
    let title: Option<String> = if post.post_number == 1 {
        sqlx::query_scalar("SELECT title FROM items WHERE id = $1")
            .bind(post.item_id)
            .fetch_optional(pool)
            .await
            .db()?
    } else {
        None
    };
    let (new_title, text) = translate(&state, Some(user), title.as_deref(), &post.raw, &to).await?;
    let cooked = save(&state, id, &to, &new_title, &text).await?;
    Ok(Json(json!({ "cooked": cooked, "machine": true, "cached": false })).into_response())
}

/// New topics into the listed languages, in the background.
pub(crate) async fn auto(state: &AppState, p: &NewPost) {
    let s = state.settings();
    if p.number != 1 || !s.bool("ai_translate_enabled") || !s.bool("content_translation_enabled") {
        return;
    }
    let wanted: Vec<String> = s
        .str("ai_auto_translate_locales")
        .split(',')
        .map(|l| l.trim().to_lowercase())
        .filter(|l| bf_core::i18n::supported(l) && l != bf_core::i18n::site_language(s.str("default_locale")))
        .collect();
    if wanted.is_empty() || p.author.starts_with("plugin-") || !may_send(state, p.node_id).await {
        return;
    }
    let Some(pool) = &state.pool else { return };
    let post_id: Option<i64> = sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = 1")
        .bind(p.topic_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some(post_id) = post_id else { return };
    for locale in wanted {
        match translations::one_current(pool, post_id, &locale).await {
            Ok(None) => {}
            _ => continue,
        }
        match translate(state, None, Some(&p.title), &p.raw, &locale).await {
            Ok((title, text)) => {
                if let Err(e) = save(state, post_id, &locale, &title, &text).await {
                    tracing::warn!(error = ?e, "automatic translation could not be saved");
                }
            }
            // The provider is down or the budget is spent: the reader's button still works later.
            Err(_) => return,
        }
    }
}
