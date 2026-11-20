//! The AI page of the admin panel: connection test, usage and cost, mood report, flagged posts,
//! embedding progress and the bot personas.

use super::client::{self, Chat};
use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_db::ai::Persona;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/ai", get(overview))
        .route("/api/v1/admin/ai/test", post(test))
        .route("/api/v1/admin/ai/embeddings/reset", post(reset))
        .route("/api/v1/admin/ai/personas", get(personas).post(create))
        .route("/api/v1/admin/ai/personas/{id}", put(update).delete(remove))
}

async fn overview(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    let pool = state.db()?;
    let s = state.settings();
    let days = 14;
    let usage = bf_db::ai::usage_report(pool, days).await.db()?;
    let (spam_at, tox_at) = (s.int("ai_spam_threshold") as i16, s.int("ai_toxicity_threshold") as i16);
    let mood = bf_db::ai::mood_report(pool, days, tox_at, spam_at).await.db()?;
    let (done, all) = bf_db::ai::embedding_count(pool).await.db()?;
    let flagged = bf_db::ai::flagged_posts(pool, tox_at, spam_at).await.db()?;
    Ok(Json(json!({
        "configured": client::configured(&state),
        "embeddings_configured": client::embeddings_configured(&state),
        "key_in_environment": std::env::var("BF_AI_API_KEY").is_ok_and(|k| !k.is_empty()),
        "tokens_this_month": bf_db::ai::tokens_this_month(pool).await.db()?,
        "token_limit": s.int("ai_monthly_token_limit"),
        "usage": usage.iter().map(|u| json!({
            "day": u.day.to_string(), "feature": u.feature, "requests": u.requests, "failures": u.failures,
            "prompt_tokens": u.prompt_tokens, "completion_tokens": u.completion_tokens,
        })).collect::<Vec<_>>(),
        "errors": bf_db::ai::recent_errors(pool).await.db()?.iter().map(|(at, feature, error)| json!({
            "at": at.to_rfc3339(), "feature": feature, "error": error,
        })).collect::<Vec<_>>(),
        "mood": mood.iter().map(|m| json!({
            "day": m.day.to_string(), "positive": m.positive, "neutral": m.neutral, "negative": m.negative,
            "toxic": m.toxic, "spam": m.spam,
        })).collect::<Vec<_>>(),
        "emotions": bf_db::ai::top_emotions(pool, days).await.db()?
            .iter().map(|(e, n)| json!({ "emotion": e, "count": n })).collect::<Vec<_>>(),
        "flagged": flagged.iter().map(|(post, topic, title, author, spam, toxicity)| json!({
            "post": post, "topic": topic, "title": title, "author": author, "spam": spam, "toxicity": toxicity,
        })).collect::<Vec<_>>(),
        "embedded": done,
        "topics": all,
    }))
    .into_response())
}

/// One small request to the chat model and, when set, to the embedding model.
async fn test(State(state): State<AppState>, Admin(..): AdminWrite) -> Result<Response, AppError> {
    if !client::configured(&state) {
        return Err(AppError::Validation(en("Turn on AI and enter the provider's address first.").into()));
    }
    let started = std::time::Instant::now();
    let done =
        client::chat(&state, Chat::new("test", "Answer with the single word OK.", "Say OK.".into()).max(10)).await?;
    let chat_ms = started.elapsed().as_millis() as u64;
    let mut out = json!({ "model": done.model, "answer": client::clip(&done.text, 80), "ms": chat_ms });
    if client::embeddings_configured(&state) {
        let started = std::time::Instant::now();
        let vectors = client::embed(&state, "test", None, &["test".to_owned()]).await?;
        out["embedding_dims"] = json!(vectors.first().map_or(0, Vec::len));
        out["embedding_ms"] = json!(started.elapsed().as_millis() as u64);
    }
    Ok(Json(out).into_response())
}

async fn reset(State(state): State<AppState>, Admin(..): AdminWrite) -> Result<Response, AppError> {
    let n = super::embeddings::reset(&state).await?;
    Ok(Json(json!({ "removed": n })).into_response())
}

async fn personas(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    Ok(Json(bf_db::ai::personas(state.db()?).await.db()?).into_response())
}

async fn check(state: &AppState, p: &Persona, id: Option<i64>) -> Result<(), AppError> {
    let bad = |m: &str| Err(AppError::Validation(m.into()));
    let ok_slug = (2..=20).contains(&p.slug.len())
        && p.slug.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    if !ok_slug {
        return bad(en("The address name takes 2 to 20 lowercase letters, digits or hyphens."));
    }
    if p.name.trim().is_empty() || p.name.chars().count() > 60 {
        return bad(en("A persona needs a name of up to 60 characters."));
    }
    if p.system_prompt.trim().is_empty() || p.system_prompt.chars().count() > 8000 {
        return bad(en("A persona needs instructions of up to 8000 characters."));
    }
    if !(50..=4000).contains(&p.max_tokens) {
        return bad(en("An answer may be 50 to 4000 tokens long."));
    }
    let pool = state.db()?;
    if bf_db::ai::persona_name_taken(pool, &p.slug).await.db()? {
        return bad(en("A member already has that name."));
    }
    let same = bf_db::ai::personas(pool).await.db()?;
    if same.iter().any(|o| o.slug == p.slug && Some(o.id) != id) {
        return bad(en("Another persona has that address name."));
    }
    Ok(())
}

async fn create(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Json(p): Json<Persona>,
) -> Result<Response, AppError> {
    check(&state, &p, None).await?;
    let id = bf_db::ai::save_persona(state.db()?, None, &p).await.db()?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
    Json(p): Json<Persona>,
) -> Result<Response, AppError> {
    check(&state, &p, Some(id)).await?;
    if bf_db::ai::save_persona(state.db()?, Some(id), &p).await.db()?.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    bf_db::ai::delete_persona(state.db()?, id).await.found()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
