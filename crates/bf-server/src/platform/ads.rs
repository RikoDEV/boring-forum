//! Advertisements: staff place ads under the header, between posts, beside topic lists,
//! in sidebars and above the footer, for chosen groups. The live list sits in memory
//! (`bf_web::ads`); impressions are counted there and saved on each tick and when the server
//! stops. A crash loses at most one tick's impressions.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_db::platform::ads::{self as db, Ad};
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ads/{id}/go", get(go))
        .route("/api/v1/admin/ads", get(list).post(create))
        .route("/api/v1/admin/ads/{id}", put(update).delete(remove))
}

const PLACES: [&str; 5] = ["header", "between_posts", "topic_list", "sidebar", "footer"];

/// The ad as pages show it (`Layout::ad_slot` adds the label), linked through the click counter when it has a link.
fn wrap(a: &Ad) -> String {
    let body = bf_web::theme::clean_html(&a.html);
    if a.url.is_some() {
        format!("<a href=\"/ads/{}/go\" rel=\"sponsored noopener\" target=\"_blank\">{body}</a>", a.id)
    } else {
        body
    }
}

/// Loads the live ads into the page store, saving the impressions counted so far first.
pub(crate) async fn reload(state: &AppState) -> Result<(), AppError> {
    let pool = state.db()?;
    flush(state).await;
    let live = db::live(pool).await.db()?;
    bf_web::ads::set(
        live.iter()
            .map(|a| {
                bf_web::ads::LiveAd::new(
                    a.id,
                    a.placement.clone(),
                    wrap(a),
                    a.show_groups.clone(),
                    a.hide_groups.clone(),
                    a.every,
                    a.weight,
                )
            })
            .collect(),
    );
    state.microcache.clear();
    Ok(())
}

/// Saves the impressions counted so far; also called as the server stops.
pub async fn flush(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    let counts = bf_web::ads::take_counts();
    if counts.is_empty() {
        return;
    }
    if let Err(e) = db::add_impressions(pool, &counts).await {
        tracing::warn!(error = %e, "saving ad impressions failed");
    }
}

/// From the maintenance tick: counts saved, and ads starting or ending since the last tick.
pub(crate) async fn run_tick(state: &AppState) {
    CLICKS_BY_IP.sweep();
    if let Err(e) = reload(state).await {
        tracing::warn!(error = ?e, "ad reload failed");
    }
}

/// Clicks counted per address: past this many a minute, the visitor is still sent on, uncounted.
static CLICKS_BY_IP: std::sync::LazyLock<crate::http::ratelimit::RateLimiter> =
    std::sync::LazyLock::new(|| crate::http::ratelimit::RateLimiter::new(10, std::time::Duration::from_secs(60)));

/// Crawlers and link checkers follow every link they find; they are not readers clicking an ad.
fn is_crawler(headers: &HeaderMap) -> bool {
    let agent = headers.get(axum::http::header::USER_AGENT).and_then(|h| h.to_str().ok()).unwrap_or_default();
    let agent = agent.to_ascii_lowercase();
    agent.is_empty()
        || ["bot", "crawl", "spider", "slurp", "preview", "fetch", "monitor"].iter().any(|k| agent.contains(k))
}

async fn go(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
    method: axum::http::Method,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let counted = method == axum::http::Method::GET
        && !crate::http::viewer::is_subresource(&headers)
        && !is_crawler(&headers)
        && v.ip.is_none_or(|ip| CLICKS_BY_IP.check(ip, std::time::Instant::now()).is_ok());
    let url = db::click(state.db()?, id, counted).await.db()?.ok_or(AppError::NotFound)?;
    Ok(Redirect::to(&url).into_response())
}

fn check(a: &mut Ad) -> Result<(), AppError> {
    let bad = |m: &str| Err(AppError::Validation(m.into()));
    a.name = a.name.trim().to_owned();
    if a.name.is_empty() || a.name.chars().count() > 100 {
        return bad(en("An ad needs a name of up to 100 characters."));
    }
    if !PLACES.contains(&a.placement.as_str()) {
        return bad(en("Choose where the ad goes."));
    }
    if a.html.trim().is_empty() || a.html.len() > 20_000 {
        return bad(en("An ad needs some HTML, up to 20,000 characters."));
    }
    // Stored cleaned, so what staff see in the list is what pages show.
    a.html = bf_web::theme::clean_html(&a.html);
    a.url = a.url.take().map(|u| u.trim().to_owned()).filter(|u| !u.is_empty());
    if let Some(u) = &a.url {
        let ok =
            (u.starts_with("https://") || u.starts_with("http://")) && url::Url::parse(u).is_ok() && u.len() <= 2000;
        if !ok {
            return bad(en("The link must be a web address starting with https://."));
        }
    }
    if !(1..=100).contains(&a.every) || !(1..=100).contains(&a.weight) {
        return bad(en("Frequency and weight are 1 to 100."));
    }
    if let (Some(s), Some(e)) = (a.starts_at, a.ends_at)
        && e <= s
    {
        return bad(en("The ad must end after it starts."));
    }
    Ok(())
}

async fn list(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(db::all(state.db()?).await.db()?).into_response())
}

async fn create(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
    headers: HeaderMap,
    Json(mut a): Json<Ad>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check(&mut a)?;
    let id = db::save(state.db()?, None, &a).await.db()?;
    reload(&state).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(mut a): Json<Ad>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check(&mut a)?;
    db::save(state.db()?, Some(id), &a).await.db()?.ok_or(AppError::NotFound)?;
    reload(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if !db::delete(state.db()?, id).await.db()? {
        return Err(AppError::NotFound);
    }
    reload(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
