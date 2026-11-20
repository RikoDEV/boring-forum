//! Watched words administration.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::moderation::watched_words;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/watched-words", get(list_words).post(add_word))
        .route("/api/v1/admin/watched-words/import", axum::routing::post(import_words))
        .route("/api/v1/admin/watched-words/{id}", axum::routing::delete(delete_word))
}

async fn list_words(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    crate::admin::api::admin(&state, &v, &headers, false)?;
    Ok(Json(watched_words::watched_words(state.db()?).await.db()?).into_response())
}

#[derive(Deserialize)]
struct NewWord {
    action: String,
    pattern: String,
    replacement: Option<String>,
}

async fn add_word(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(w): Json<NewWord>,
) -> Result<Response, AppError> {
    let user_id = crate::admin::api::admin(&state, &v, &headers, true)?.unwrap_or(0);
    let pattern = w.pattern.trim();
    if !matches!(
        w.action.as_str(),
        "block" | "censor" | "require_approval" | "replace" | "link" | "tag" | "flag" | "silence"
    ) || pattern.is_empty()
        || pattern.len() > 100
        || pattern.chars().any(char::is_whitespace)
    {
        return Err(AppError::Validation(
            "action must be block, censor, require_approval, replace, link, tag, flag or silence; pattern is one word"
                .into(),
        ));
    }
    if w.action == "replace" && w.replacement.as_deref().is_none_or(str::is_empty) {
        return Err(AppError::Validation("replace needs a replacement".into()));
    }
    let pool = state.db()?;
    let id = watched_words::add_watched_word(pool, &w.action, pattern, w.replacement.as_deref()).await.db()?;
    bf_db::admin::audit::record(
        pool,
        bf_db::admin::audit::Entry::new("watched_word.add")
            .by(user_id)
            .on(format!("watched_word:{id}"))
            .details(json!({ "action": w.action, "pattern": pattern })),
    )
    .await
    .db()?;
    state.forum.invalidate();
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

const ACTIONS: [&str; 8] = ["block", "censor", "require_approval", "replace", "link", "tag", "flag", "silence"];
const MOST_WORDS: usize = 10_000;

#[derive(Deserialize)]
struct Import {
    action: String,
    replacement: Option<String>,
    /// The file's text: words separated by new lines, commas or semicolons. `#` starts a comment line.
    text: String,
}

/// Imports a list of words from a text file, all with the one action chosen for them.
async fn import_words(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<Import>,
) -> Result<Response, AppError> {
    let user_id = crate::admin::api::admin(&state, &v, &headers, true)?.unwrap_or(0);
    if !ACTIONS.contains(&b.action.as_str()) {
        return Err(AppError::Validation("Pick an action for the words.".into()));
    }
    if b.action == "replace" && b.replacement.as_deref().is_none_or(str::is_empty) {
        return Err(AppError::Validation("replace needs a replacement".into()));
    }
    let replacement = b.replacement.as_deref().filter(|r| !r.is_empty());
    let (mut words, mut skipped) = (std::collections::BTreeSet::new(), 0_i64);
    for line in b.text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for token in line.split([',', ';']) {
            let word = token.trim();
            if word.is_empty() {
                continue;
            }
            // A watched word is one word, up to 100 characters.
            if word.chars().count() > 100 || word.chars().any(char::is_whitespace) {
                skipped += 1;
            } else {
                words.insert(word.to_lowercase());
            }
        }
    }
    if words.is_empty() {
        return Err(AppError::Validation("The file has no words in it.".into()));
    }
    if words.len() > MOST_WORDS {
        return Err(AppError::Validation("Too many words in one file. Split it in parts of 10,000.".into()));
    }
    let words: Vec<String> = words.into_iter().collect();
    let pool = state.db()?;
    let (added, existing) = watched_words::import_watched_words(pool, &b.action, replacement, &words).await.db()?;
    bf_db::admin::audit::record(
        pool,
        bf_db::admin::audit::Entry::new("watched_word.import")
            .by(user_id)
            .on("watched_word:import")
            .details(json!({ "action": b.action, "added": added, "existing": existing, "skipped": skipped })),
    )
    .await
    .db()?;
    state.forum.invalidate();
    Ok(Json(json!({ "added": added, "existing": existing, "skipped": skipped })).into_response())
}

async fn delete_word(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let user_id = crate::admin::api::admin(&state, &v, &headers, true)?.unwrap_or(0);
    let pool = state.db()?;
    if !watched_words::delete_watched_word(pool, id).await.db()? {
        return Err(AppError::NotFound);
    }
    bf_db::admin::audit::record(
        pool,
        bf_db::admin::audit::Entry::new("watched_word.delete").by(user_id).on(format!("watched_word:{id}")),
    )
    .await
    .db()?;
    state.forum.invalidate();
    Ok(StatusCode::NO_CONTENT.into_response())
}
