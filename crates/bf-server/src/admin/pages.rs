//! Custom pages: written in the admin panel, served at `/page/{slug}` or at an address of their
//! own (`path`, such as `/about`). Each page has widget areas (`page{id}_top|side|bottom`),
//! arranged with the layout editor: the page builder. This module keeps the map of custom
//! addresses for the 404 fallback.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::{DbResult, db_err};
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::slug::slugify;
use bf_db::admin::pages;
use serde::Deserialize;
use serde_json::json;
use std::sync::{LazyLock, RwLock};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/page/{slug}", get(show))
        .route("/api/v1/admin/pages", get(list).post(create))
        .route("/api/v1/admin/pages/{id}", put(update).delete(remove))
}

/// Custom addresses: (path, page slug).
static PATHS: LazyLock<RwLock<Vec<(String, String)>>> = LazyLock::new(Default::default);

/// At startup and after a page changes.
pub(crate) async fn load_paths(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    match pages::custom_paths(pool).await {
        Ok(rows) => *PATHS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = rows,
        Err(e) => tracing::warn!(error = %e, "custom addresses failed to load"),
    }
}

/// Top-level segments the site's own routes use; a custom address may not start with one.
const RESERVED: &[&str] = &[
    "about",
    "admin",
    "ads",
    "ap",
    "api",
    "assets",
    "auth",
    "avatar",
    "badges",
    "c",
    "chat",
    "clubs",
    "confirm-email",
    "contact",
    "g",
    "groups",
    "healthz",
    "language",
    "latest",
    "leaderboard",
    "legal",
    "live",
    "livez",
    "login",
    "logout",
    "manifest.webmanifest",
    "messages",
    "metrics",
    "new",
    "newsletter",
    "nodeinfo",
    "notifications",
    "oauth",
    "oembed",
    "p",
    "page",
    "post",
    "r",
    "readyz",
    "referrals",
    "register",
    "review",
    "robots.txt",
    "safe-mode",
    "scheduled",
    "search",
    "settings",
    "sitemap.xml",
    "store",
    "sudo",
    "support",
    "sw.js",
    "t",
    "tag",
    "tags",
    "top",
    "trust",
    "transparency",
    "translate",
    "u",
    "unread",
    "uploads",
    "welcome",
];

/// A custom address as typed: empty for none, otherwise `/a`, `/a/b` or `/a/b/c` in lowercase
/// letters, digits and dashes, not under one of the site's own routes.
pub(crate) fn check_path(raw: &str) -> Result<Option<String>, AppError> {
    let p = raw.trim().trim_end_matches('/').to_lowercase();
    if p.is_empty() {
        return Ok(None);
    }
    let p = if p.starts_with('/') { p } else { format!("/{p}") };
    let segments: Vec<&str> = p[1..].split('/').collect();
    let ok = segments.len() <= 3
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.len() <= 60
                && !s.starts_with('-')
                && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
    if !ok {
        return Err(AppError::Validation(
            "An address is up to three parts of lowercase letters, digits and dashes, like /about or /help/rules."
                .into(),
        ));
    }
    if RESERVED.contains(&segments[0]) || segments[0].starts_with('.') {
        return Err(AppError::invalid(
            "/{n} is used by the site itself; choose another address.",
            &[("n", &segments[0])],
        ));
    }
    Ok(Some(p))
}

/// The 404 fallback: a custom address serves its page.
pub(crate) async fn custom_path(state: &AppState, v: Viewer, uri: &axum::http::Uri) -> Option<Response> {
    let path = uri.path().trim_end_matches('/').to_lowercase();
    let hit =
        PATHS.read().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(p, _)| *p == path).cloned()?;
    let q: crate::http::ui::PageQuery = Query::try_from_uri(uri).map(|q| q.0).unwrap_or_default();
    let res = render_page(state, &v, &hit.1, q.editing(), true).await;
    Some(res.unwrap_or_else(IntoResponse::into_response))
}

async fn show(
    State(state): State<AppState>,
    v: Viewer,
    Path(slug): Path<String>,
    Query(q): Query<crate::http::ui::PageQuery>,
) -> Result<Response, AppError> {
    render_page(&state, &v, &slug, q.editing(), false).await
}

/// A custom page with its widget areas. Reached by `/page/{slug}`, it sends visitors on to the
/// page's own address when it has one.
pub(crate) async fn render_page(
    state: &AppState,
    v: &Viewer,
    slug: &str,
    editing: bool,
    via_path: bool,
) -> Result<Response, AppError> {
    use crate::http::ui::{Shell, begin, finish};
    let p = pages::by_slug(state.db()?, slug).await.db()?.ok_or(AppError::NotFound)?;
    if let (Some(path), false) = (&p.path, via_path) {
        return Ok(axum::response::Redirect::permanent(path).into_response());
    }
    let staff = is_staff(v);
    // A draft looks like it does not exist, except to staff checking it.
    if !p.published && !staff {
        return Err(AppError::NotFound);
    }
    let allowed = match p.visibility.as_str() {
        "members" => v.user.is_some(),
        "staff" => staff,
        _ => true,
    };
    if !allowed {
        v.require_user()?;
        return Err(AppError::Forbidden);
    }
    let ctx = begin(state, v);
    let canonical = p.path.clone().unwrap_or_else(|| format!("/page/{}", p.slug));
    // The page is its blocks: the layout (`template`) says where they go.
    let shell = Shell {
        title: p.title.clone(),
        canonical: Some(canonical),
        noindex: !p.published || p.visibility != "public",
        page_template: p.template.clone(),
        no_side: !pages::places(&p.template).contains(&"side"),
        ..Shell::default()
    }
    .with_area(state, &format!("page{}", p.id), editing)
    .await;
    Ok(finish(state, v, ctx, shell, String::new()))
}

async fn list(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    Ok(Json(pages::all(state.db()?).await.db()?).into_response())
}

#[derive(Deserialize)]
struct PageBody {
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    template: String,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    published: bool,
    #[serde(default)]
    path: String,
}

struct Checked {
    slug: String,
    title: String,
    template: String,
    path: Option<String>,
}

fn check(b: &PageBody) -> Result<Checked, AppError> {
    let title = b.title.trim();
    if title.is_empty() || title.chars().count() > 150 {
        return Err(AppError::Validation("A page needs a title of up to 150 characters.".into()));
    }
    if !matches!(b.visibility.as_str(), "public" | "members" | "staff") {
        return Err(AppError::Validation("Visibility must be public, members or staff.".into()));
    }
    let slug = slugify(if b.slug.trim().is_empty() { title } else { b.slug.trim() });
    if slug.is_empty() {
        return Err(AppError::Validation(
            "That title has no characters usable in an address; set one yourself.".into(),
        ));
    }
    let template = if b.template.is_empty() { "single" } else { b.template.as_str() };
    if !pages::TEMPLATES.contains(&template) {
        return Err(AppError::Validation("Pick one of the page layouts.".into()));
    }
    Ok(Checked { slug, title: title.to_owned(), template: template.to_owned(), path: check_path(&b.path)? })
}

fn taken(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.is_unique_violation())
}

/// One line of the staff log for a custom page.
async fn audit(
    state: &AppState,
    who: Option<i64>,
    action: &str,
    id: i64,
    c: Option<(&Checked, &PageBody)>,
) -> Result<(), AppError> {
    let details = match c {
        Some((c, b)) => json!({
            "slug": c.slug, "title": c.title, "template": c.template, "path": c.path,
            "visibility": b.visibility, "published": b.published,
        }),
        None => json!({}),
    };
    crate::admin::api::audit_log(state.db()?, who, action, &format!("page:{id}"), details).await
}

async fn create(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<PageBody>,
) -> Result<Response, AppError> {
    let c = check(&b)?;
    let change = pages::PageChange {
        slug: &c.slug,
        title: &c.title,
        template: &c.template,
        visibility: &b.visibility,
        published: b.published,
        path: c.path.as_deref(),
    };
    match pages::create(state.db()?, &change).await {
        Ok(id) => {
            audit(&state, who, "page.create", id, Some((&c, &b))).await?;
            load_paths(&state).await;
            Ok((StatusCode::CREATED, Json(json!({ "id": id, "slug": c.slug }))).into_response())
        }
        Err(e) if taken(&e) => Err(AppError::Validation("Another page already uses that slug or address.".into())),
        Err(e) => Err(db_err(e)),
    }
}

async fn update(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<PageBody>,
) -> Result<Response, AppError> {
    let c = check(&b)?;
    let change = pages::PageChange {
        slug: &c.slug,
        title: &c.title,
        template: &c.template,
        visibility: &b.visibility,
        published: b.published,
        path: c.path.as_deref(),
    };
    match pages::update(state.db()?, id, &change).await {
        Ok(true) => {
            audit(&state, who, "page.update", id, Some((&c, &b))).await?;
            pages::move_stray_widgets(state.db()?, id, &c.template).await.db()?;
            state.widgets.invalidate_all();
            state.widget_list.invalidate_all();
            load_paths(&state).await;
            state.microcache.clear();
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        Ok(false) => Err(AppError::NotFound),
        Err(e) if taken(&e) => Err(AppError::Validation("Another page already uses that slug or address.".into())),
        Err(e) => Err(db_err(e)),
    }
}

async fn remove(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    pages::delete(state.db()?, id).await.found()?;
    audit(&state, who, "page.delete", id, None).await?;
    load_paths(&state).await;
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}
