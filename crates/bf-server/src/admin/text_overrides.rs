//! Interface text overrides.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::admin::text_overrides;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/text", get(list_text))
        .route("/api/v1/admin/text/{locale}/{key}", put(set_text).delete(delete_text))
}

/// Registers the texts kept in the database (badges, forums, announcements, navigation labels...)
/// so they can be translated; true when the set changed.
async fn register_site_texts(pool: &sqlx::PgPool) -> Result<bool, sqlx::Error> {
    let mut texts = text_overrides::site_texts(pool).await?;
    // Custom navigation links (the ones that are not a built-in module).
    for source in text_overrides::navigation_sources(pool).await? {
        texts.extend(bf_core::nav::parse(&source).into_iter().filter(|i| i.key.is_none()).map(|i| i.label));
    }
    Ok(bf_core::i18n::set_site_texts(texts))
}

/// Registers the texts kept in the database again and applies the overrides: after a badge or a
/// forum is saved, so its new text can be translated in Site text.
pub(crate) async fn refresh(state: &AppState) {
    if let Some(pool) = &state.pool {
        if let Err(e) = load_overrides(pool).await {
            tracing::warn!(error = %e, "site texts not registered");
        }
        state.microcache.clear();
    }
}

pub(crate) async fn load_overrides(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    register_site_texts(pool).await?;
    bf_core::i18n::set_overrides(&text_overrides::text_overrides(pool).await?);
    Ok(())
}

#[derive(Deserialize)]
struct ListQuery {
    locale: Option<String>,
}

/// The catalog for the editor: each key with its English text and `locale`'s own built-in text,
/// plus every override and the languages the site speaks.
async fn list_text(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Query(q): Query<ListQuery>,
) -> Result<Response, AppError> {
    let locale = q.locale.filter(|l| bf_core::i18n::supported(l)).unwrap_or_else(|| "en".into());
    let pool = state.db()?;
    // Texts written since the last visit (a new badge, an announcement) are listed too.
    if register_site_texts(pool).await.db()? {
        bf_core::i18n::set_overrides(&text_overrides::text_overrides(pool).await.db()?);
    }
    let overrides = text_overrides::text_overrides(pool).await.db()?;
    let out: Vec<_> = overrides.into_iter().map(|(l, k, t)| json!({ "locale": l, "key": k, "value": t })).collect();
    let own: std::collections::HashMap<_, _> = bf_core::i18n::catalog_for(&locale).into_iter().collect();
    // Texts from the database (a custom badge's name) are listed with the built-in ones.
    let catalog: Vec<_> = bf_core::i18n::catalog()
        .into_iter()
        .chain(bf_core::i18n::site_catalog())
        .map(|(k, t)| {
            let default = own.get(&k).cloned().unwrap_or_else(|| t.clone());
            json!({ "key": k, "text": t, "default": default })
        })
        .collect();
    let locales: Vec<_> = bf_core::i18n::locales().into_iter().map(|(c, n)| json!({ "code": c, "name": n })).collect();
    Ok(Json(json!({ "overrides": out, "catalog": catalog, "locales": locales, "locale": locale })).into_response())
}

#[derive(Deserialize)]
struct TextBody {
    value: String,
}

async fn set_text(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path((locale, key)): Path<(String, String)>,
    Json(b): Json<TextBody>,
) -> Result<Response, AppError> {
    let known = bf_core::i18n::catalog().iter().any(|(k, _)| *k == key)
        || bf_core::i18n::site_catalog().iter().any(|(k, _)| *k == key);
    if !bf_core::i18n::supported(&locale) || !known {
        return Err(AppError::Validation("Pick a known language and text.".into()));
    }
    bf_core::i18n::validate_override(&b.value).map_err(AppError::Validation)?;
    crate::admin::mail_templates::check_override(&key, &b.value)?;
    let pool = state.db()?;
    text_overrides::set_text_override(pool, &locale, &key, &b.value).await.db()?;
    // Site text includes every mail the site sends, so a rewrite is on record with its words.
    let target = format!("text:{locale}:{key}");
    crate::admin::api::audit_log(pool, who, "text.set", &target, json!({ "value": b.value })).await?;
    load_overrides(pool).await.db()?;
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_text(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path((locale, key)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    text_overrides::delete_text_override(pool, &locale, &key).await.found()?;
    crate::admin::api::audit_log(pool, who, "text.delete", &format!("text:{locale}:{key}"), json!({})).await?;
    load_overrides(pool).await.db()?;
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}
