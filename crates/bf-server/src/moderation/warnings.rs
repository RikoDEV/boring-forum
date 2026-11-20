//! Warnings.

use crate::http::i18n::trh;
use crate::http::render::{ago, DbResult};
use crate::http::ui::simple_page;
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_db::community::promotion;
use bf_db::moderation::warnings;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/u/{id}/warn", post(warn)).route("/u/{id}/warnings", get(warnings))
}

#[derive(Deserialize)]
struct WarnForm {
    csrf: String,
    points: i32,
    reason: String,
    /// Where to return to when the warning was given from the moderator panel.
    #[serde(default)]
    back: String,
}

async fn warn(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<WarnForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    issue(&state, &v, id, f.points, &f.reason).await?;
    let url = format!("/u/{id}/warnings");
    if f.back.starts_with("/modcp") {
        return Ok(Redirect::to(&crate::moderation::discipline::back_to(&f.back, "warn")).into_response());
    }
    Ok(Redirect::to(&url).into_response())
}

/// Gives a member a warning, applies the automatic action the points reach, and tells them.
/// For staff only; never on oneself, an administrator, or (for a moderator) another moderator.
pub(crate) async fn issue(state: &AppState, v: &Viewer, id: i64, points: i32, reason: &str) -> Result<(), AppError> {
    let user = v.require_user()?;
    let pool = state.db()?;
    if !is_staff(v) {
        return Err(AppError::Forbidden);
    }
    let target =
        bf_db::accounts::users::user_by_id(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    if target.id == user.id || target.is_admin {
        return Err(AppError::Validation("You cannot warn yourself or an administrator.".into()));
    }
    // Enough points suspend: a moderator must not be able to suspend another moderator.
    if !v.actor.is_admin && target.groups.contains(&bf_core::ids::GroupId::MODERATORS) {
        return Err(AppError::Validation("Only an administrator can warn a moderator.".into()));
    }
    let reason = reason.trim();
    if !(1..=100).contains(&points) || reason.is_empty() || reason.chars().count() > 500 {
        return Err(AppError::Validation("A warning needs 1 to 100 points and a reason.".into()));
    }
    let s = state.settings();
    let t = promotion::Thresholds {
        silence_points: s.int("warning_silence_points"),
        suspend_points: s.int("warning_suspend_points"),
        days: s.int("warning_action_days") as i32,
    };
    let (_, was_silenced, was_suspended) = warnings::warning_state(pool, id).await.db()?;
    warnings::warn(pool, id, user.id.get(), points, reason, s.int("warning_expire_days") as i32, &t)
        .await
        .db()?;
    let (_, silenced, suspended) = warnings::warning_state(pool, id).await.db()?;
    let url = format!("/u/{id}/warnings");
    let data = json!({ "message": reason, "url": url });
    crate::community::notifications::send(state, 0, &[id], "warning", &data).await;
    // The points crossed a threshold: the member hears what happened to their account and gets
    // a statement of reasons, as when staff restrict it by hand.
    for (now, before, action) in [(silenced, was_silenced, "silenced"), (suspended, was_suspended, "suspended")] {
        if now && !before {
            crate::community::notifications::send(state, 0, &[id], action, &json!({ "url": "/settings" })).await;
            crate::moderation::dsa::record(
                state,
                crate::moderation::dsa::Restriction {
                    user_id: id,
                    target_type: "account",
                    target_id: id,
                    action,
                    ground: "terms",
                    category: "scope_of_platform_service",
                    facts: "Active warning points reached the limit the site sets for this action.",
                    automated_detection: false,
                    automated_decision: true,
                    notice_id: None,
                    moderator_id: Some(user.id.get()),
                },
            )
            .await;
        }
    }
    Ok(())
}

/// Seen by the member and by staff only.
async fn warnings(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let pool = state.db()?;
    if user.id != id && !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let rows = warnings::warnings_of(pool, id).await.db()?;
    let now = chrono::Utc::now();
    let items: String = rows
        .iter()
        .map(|w| {
            let state = if w.expires_at > now { "active" } else { "expired" };
            format!(
                "<li><strong>{} points</strong> · {} · {state} · by {}<p class=\"small\">{}</p></li>",
                w.points,
                ago(w.created_at),
                esc(w.moderator.as_deref().unwrap_or("staff")),
                esc(&w.reason)
            )
        })
        .collect();
    let html = if items.is_empty() {
        format!("<p class=\"muted\">{tr1}</p>", tr1 = trh("No warnings."))
    } else {
        format!("<ul class=\"activity\">{items}</ul>")
    };
    Ok(simple_page(&state, &v, &v.t().msg("Warnings"), &html, false))
}
