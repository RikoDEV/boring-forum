//! The admin dashboard: error and task logs, version check, test mail, style guide.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::admin::maintenance;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/styleguide", get(styleguide))
        .route("/api/v1/admin/logs/errors", get(error_log))
        .route("/api/v1/admin/logs/tasks", get(task_log))
        .route("/api/v1/admin/dashboard", get(dashboard))
        .route("/api/v1/admin/mail/test", post(test_mail))
}

/// `GET /modcp`: overview, hidden content and IP tools for the forums the viewer moderates.
/// The component catalog, for staff checking a theme.
async fn styleguide(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    v.require_user()?;
    if !crate::http::viewer::is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let (mut l, set) = crate::http::render::layout(&state, &v, Some(&v.t().msg("Styleguide")));
    l.noindex = true;
    let icons = vec![
        "bell",
        "mail",
        "search",
        "menu",
        "message-square",
        "eye",
        "pin",
        "lock",
        "circle-check",
        "heart",
        "reply",
        "quote",
        "bookmark",
        "flag",
        "pencil",
        "trash-2",
        "ellipsis",
        "settings",
        "user",
    ];
    Ok(crate::http::render::page(
        bf_web::pages::render(&bf_web::pages::StyleguidePage { layout: l, icons }),
        set,
        false,
    ))
}

async fn error_log(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    Ok(Json(maintenance::errors(state.db()?).await.db()?).into_response())
}

async fn task_log(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    Ok(Json(maintenance::tasks(state.db()?).await.db()?).into_response())
}

/// Dashboard extras: problems, version, latest staff actions.
async fn dashboard(State(state): State<AppState>, Admin(_, v): AdminRead) -> Result<Response, AppError> {
    let pool = state.db()?;
    let (mail, bad, errors, pending) = maintenance::problems(pool).await.db()?;
    let mut problems = Vec::new();
    let mut note = |n: i64, text: String, link: &str| {
        if n > 0 {
            problems.push(json!({ "text": text, "link": link }));
        }
    };
    let t = v.t();
    note(mail, t.n("admin.problem_mail", mail), "/admin/mail/");
    note(bad, t.n("admin.problem_bad_email", bad), "/admin/mail/");
    note(errors, t.n("admin.problem_errors", errors), "/admin/logs/");
    note(pending, t.n("admin.problem_pending", pending), "/review");
    if std::env::var("BF_SMTP_URL").is_err() && std::env::var("BF_MAIL_DIR").is_err() {
        problems
            .push(json!({ "text": t.msg("No mail transport is configured, so email is not delivered"), "link": "" }));
    }
    let release = fetch_latest_version(state.settings().str("version_check_url")).await;
    let current = env!("CARGO_PKG_VERSION");
    let latest = release.as_ref().map(|r| r.version.clone());
    if let Some(r) = release.as_ref().filter(|r| r.version != current) {
        // An update notice for administrators, and a release carrying a security fix is
        // marked as such rather than looking like any other version bump.
        let text = if r.security {
            t.msgf(
                "Version {version} is available and contains a SECURITY fix; this is {current}",
                &[("version", &r.version), ("current", current)],
            )
        } else {
            t.msgf(
                "Version {version} is available; this is {current}",
                &[("version", &r.version), ("current", current)],
            )
        };
        problems.push(json!({ "text": text, "link": r.advisory, "security": r.security }));
    }
    let actions = maintenance::latest_staff_actions(pool).await.db()?;
    Ok(Json(json!({ "version": current, "latest": latest, "problems": problems, "actions": actions })).into_response())
}

/// What the version-check URL says about the newest release.
pub(crate) struct Release {
    pub version: String,
    /// True when this release fixes a security issue, so the notice can say so.
    pub security: bool,
    /// Advisory or release-notes URL, shown as the notice's link. Empty when not given.
    pub advisory: String,
}

/// The latest release from the configured URL. Off when the setting is empty.
///
/// Two accepted formats. Plain text is just the version, as before. JSON marks a security
/// release:
///
/// ```json
/// { "version": "0.2.1", "security": true, "advisory": "https://…/GHSA-xxxx" }
/// ```
///
/// Fetched through the guarded client, so an admin who points this at an internal
/// address cannot use it to probe the private network.
async fn fetch_latest_version(url: &str) -> Option<Release> {
    if url.is_empty() {
        return None;
    }
    let bytes = crate::platform::outbound::fetch_public(url, 8 * 1024).await.ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let trimmed = text.trim();

    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(version) = v.get("version").and_then(|x| x.as_str())
    {
        return Some(Release {
            version: version.trim().chars().take(40).collect(),
            security: v.get("security").and_then(serde_json::Value::as_bool).unwrap_or(false),
            // Only an https link is shown, so the notice cannot become a javascript: sink.
            advisory: v
                .get("advisory")
                .and_then(|x| x.as_str())
                .filter(|a| a.starts_with("https://"))
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect(),
        });
    }
    Some(Release { version: trimmed.chars().take(40).collect(), security: false, advisory: String::new() })
}

#[derive(Deserialize)]
struct TestMail {
    to: String,
}

async fn test_mail(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Json(b): Json<TestMail>,
) -> Result<Response, AppError> {
    let to = b.to.trim();
    if !to.contains('@') || to.contains(char::is_whitespace) {
        return Err(AppError::Validation("Enter an email address.".into()));
    }
    let site = state.settings().str("site_name").to_owned();
    bf_db::platform::mail::queue_mail(
        state.db()?,
        to,
        &format!("Test email from {site}"),
        "If you can read this, outgoing mail works.",
    )
    .await
    .db()?;
    Ok(StatusCode::ACCEPTED.into_response())
}
