//! Custom emoji.

use crate::admin::api::{Admin, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{Json, Router};
use bf_core::AppError;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/emoji", post(add_emoji))
        .route("/api/v1/admin/emoji/{name}", delete(delete_emoji))
}

/// Reloads the cooker's custom emoji from the database.
pub(crate) async fn load_custom_emoji(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let rows = bf_db::admin::emoji::custom_emoji(pool).await?;
    bf_cook::set_custom_emoji(rows.into_iter().collect());
    Ok(())
}

/// Posts already cooked hold the old emoji (an image, or the plain text), so they are cooked again
/// in the background, and pages cached with them are dropped.
async fn recook_everything(state: &AppState) -> Result<(), AppError> {
    bf_db::admin::maintenance::mark_all_for_recook(state.db()?).await.db()?;
    tokio::spawn(crate::state::recook(state.clone()));
    state.microcache.clear();
    Ok(())
}

#[derive(Deserialize)]
struct NewEmoji {
    name: String,
    url: String,
}

async fn add_emoji(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<NewEmoji>,
) -> Result<Response, AppError> {
    let name = b.name.trim().trim_matches(':').to_lowercase();
    let ok_name =
        !name.is_empty() && name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let path = b.url.split('#').next().unwrap_or_default();
    let ok_url = path
        .strip_prefix("/uploads/")
        .is_some_and(|k| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') && k.contains('.'));
    if !ok_name || !ok_url {
        return Err(AppError::Validation("Use a short name of letters and digits, and an uploaded image.".into()));
    }
    let pool = state.db()?;
    bf_db::admin::emoji::set_custom_emoji(pool, &name, path).await.db()?;
    crate::admin::api::audit_log(pool, who, "emoji.set", &format!("emoji:{name}"), json!({ "url": path })).await?;
    load_custom_emoji(pool).await.db()?;
    recook_everything(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_emoji(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    bf_db::admin::emoji::delete_custom_emoji(pool, &name).await.found()?;
    crate::admin::api::audit_log(pool, who, "emoji.delete", &format!("emoji:{name}"), json!({})).await?;
    load_custom_emoji(pool).await.db()?;
    recook_everything(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
