//! Legal documents (view, accept, publish), likes and "top" lists.

use crate::http::render::{DbResult, layout, page};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_db::community::legal;
use bf_web::pages::{LegalPage, render};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/legal/{kind}", get(legal))
        .route("/legal/accept", get(accept_form).post(accept))
        .route("/api/v1/admin/legal/{kind}", get(current).put(publish))
}

/// Read at startup, so the footer links only the documents that exist.
pub(crate) async fn load_published(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    match legal::published_kinds(pool).await {
        Ok(kinds) => bf_web::pages::LegalDocs::set(kinds.iter().map(String::as_str)),
        Err(e) => tracing::warn!(error = %e, "legal document load failed"),
    }
}

pub(crate) fn legal_title(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "terms" => bf_core::i18n::en("Terms of service"),
        "privacy" => bf_core::i18n::en("Privacy policy"),
        "guidelines" => bf_core::i18n::en("Community guidelines"),
        _ => return None,
    })
}

async fn legal(State(state): State<AppState>, v: Viewer, Path(kind): Path<String>) -> Result<Response, AppError> {
    let title = v.t().msg(legal_title(&kind).ok_or(AppError::NotFound)?);
    let doc = legal::latest(state.db()?, &kind).await.db()?.ok_or(AppError::NotFound)?;
    let (mut l, set) = layout(&state, &v, Some(&title));
    l.canonical = Some(format!("{}/legal/{kind}", state.config.base_url));
    let body = LegalPage {
        layout: l,
        title,
        updated: bf_web::ago::Ago::day(doc.created_at),
        version: doc.version,
        html: doc.body_html,
        accept: vec![],
    };
    Ok(page(render(&body), set, v.user.is_none()))
}

async fn accept_form(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let docs = legal::pending(state.db()?, user.id.get()).await.db()?;
    if docs.is_empty() {
        return Ok(Redirect::to("/").into_response());
    }
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Updated terms")));
    l.noindex = true;
    // This page is the dialog's content, and has the form itself.
    l.legal_pending = false;
    let html = docs
        .iter()
        .map(|d| format!("<h2>{}</h2>{}", crate::http::i18n::trh(legal_title(&d.kind).unwrap_or("")), d.body_html))
        .collect();
    let body = LegalPage {
        layout: l,
        title: "Please review our updated terms".into(),
        updated: bf_web::ago::Ago::default(),
        version: 0,
        html,
        accept: docs.iter().map(|d| d.id).collect(),
    };
    Ok(page(render(&body), set, false))
}

#[derive(Deserialize)]
struct AcceptForm {
    csrf: String,
    docs: String,
    /// The page the terms dialog was opened on, to return to it.
    #[serde(default)]
    next: String,
}

async fn accept(State(state): State<AppState>, v: Viewer, Form(f): Form<AcceptForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    // Only documents that are actually pending can be accepted.
    let pending: Vec<i64> = legal::pending(pool, user.id.get()).await.db()?.iter().map(|d| d.id).collect();
    let ids: Vec<i64> =
        f.docs.split(',').filter_map(|s| s.trim().parse().ok()).filter(|id| pending.contains(id)).collect();
    legal::accept(pool, user.id.get(), &ids).await.db()?;
    let next = if crate::platform::plugins::local_path(&f.next) { f.next.as_str() } else { "/" };
    Ok(Redirect::to(next).into_response())
}

/// The latest version's Markdown, for the editor; an unpublished document is empty.
async fn current(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Result<Response, AppError> {
    crate::admin::api::admin(&state, &v, &headers, false)?;
    legal_title(&kind).ok_or(AppError::NotFound)?;
    let doc = legal::latest(state.db()?, &kind).await.db()?;
    Ok(Json(json!({
        "body": doc.as_ref().map_or("", |d| d.body_md.as_str()),
        "version": doc.as_ref().map_or(0, |d| d.version),
    }))
    .into_response())
}

#[derive(Deserialize)]
struct PublishBody {
    body: String,
}

async fn publish(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(kind): Path<String>,
    Json(b): Json<PublishBody>,
) -> Result<Response, AppError> {
    let who = crate::admin::api::admin(&state, &v, &headers, true)?;
    legal_title(&kind).ok_or(AppError::NotFound)?;
    if b.body.trim().is_empty() || b.body.len() > 200_000 {
        return Err(AppError::Validation("The document needs text (at most 200 KB).".into()));
    }
    let html = bf_cook::cook(&b.body).html;
    let version = legal::publish(state.db()?, &kind, &b.body, &html, who.unwrap_or(0)).await.db()?;
    bf_web::pages::LegalDocs::mark(&kind);
    state.microcache.clear();
    Ok((StatusCode::CREATED, Json(json!({ "version": version }))).into_response())
}
