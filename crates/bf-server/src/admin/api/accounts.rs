//! Member search and the edits an admin makes to one member.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::{DbResult, db_err};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_db::admin::members;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::actor;

#[derive(Deserialize)]
pub(super) struct UserQuery {
    #[serde(default)]
    pub(super) q: String,
    #[serde(default)]
    pub(super) status: String,
    #[serde(default)]
    pub(super) sort: String,
    #[serde(default)]
    pub(super) page: i64,
    pub(super) id: Option<i64>,
}

pub(super) const MEMBERS_PAGE: i64 = 50;

pub(super) async fn users(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Query(q): Query<UserQuery>,
) -> Result<Response, AppError> {
    let text: String = q.q.trim().chars().take(100).collect();
    let filter = members::UserFilter { q: &text, status: &q.status, sort: &q.sort, id: q.id };
    let offset = (q.page.max(1) - 1) * MEMBERS_PAGE;
    Ok(Json(members::users(state.db()?, &filter, MEMBERS_PAGE, offset).await.db()?).into_response())
}

/// `null` clears a date; a missing key leaves it unchanged.
pub(super) fn double_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
pub(super) struct UserPatch {
    pub(super) primary_group_id: Option<i64>,
    pub(super) secondary_groups: Option<Vec<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub(super) suspended_until: Option<Option<DateTime<Utc>>>,
    #[serde(default, deserialize_with = "double_option")]
    pub(super) silenced_until: Option<Option<DateTime<Utc>>>,
    pub(super) approved: Option<bool>,
    pub(super) trust_level: Option<i16>,
    pub(super) trust_level_locked: Option<bool>,
    pub(super) email_verified: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    pub(super) title: Option<Option<String>>,
    /// Why the account is suspended or silenced; goes into the statement of reasons.
    #[serde(default)]
    pub(super) reason: Option<String>,
}

pub(super) async fn end_sessions(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    if who == Some(id) {
        return Err(AppError::Validation("Sign out from your own settings instead.".into()));
    }
    members::end_sessions(state.db()?, id, actor(who)).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn update_user(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(p): Json<UserPatch>,
) -> Result<Response, AppError> {
    if who == Some(id) && (p.primary_group_id.is_some_and(|g| g != 4) || p.suspended_until.flatten().is_some()) {
        return Err(AppError::Validation("You cannot demote or suspend yourself.".into()));
    }
    let top = state.settings().trust_levels().max_level();
    if p.trust_level.is_some_and(|l| !(0..=top).contains(&l)) {
        return Err(AppError::invalid("A trust level is 0 to {top}.", &[("top", &top.to_string())]));
    }
    let change = members::UserChange {
        trust_level: p.trust_level,
        trust_level_locked: p.trust_level_locked,
        email_verified: p.email_verified,
        primary_group_id: p.primary_group_id,
        secondary_groups: p.secondary_groups,
        suspended_until: p.suspended_until,
        silenced_until: p.silenced_until,
        approved: p.approved,
        title: p.title.map(|t| t.map(|s| s.trim().chars().take(60).collect::<String>()).filter(|s| !s.is_empty())),
    };
    let pool = state.db()?;
    let was_pending = p.approved == Some(true)
        && sqlx::query_scalar::<_, bool>("SELECT NOT approved FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .db()?
            .unwrap_or(false);
    let ok = match members::update_user(pool, id, &change, actor(who)).await {
        Ok(ok) => ok,
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
            return Err(AppError::Validation("Unknown group.".into()));
        }
        Err(e) => return Err(db_err(e)),
    };
    if !ok {
        return Err(AppError::NotFound);
    }
    state.microcache.clear();
    let by = actor(who);
    let mut notices = Vec::new();
    match p.suspended_until {
        Some(Some(_)) => notices.push("suspended"),
        Some(None) => notices.push("restored"),
        None => {}
    }
    match p.silenced_until {
        Some(Some(_)) => notices.push("silenced"),
        Some(None) if !notices.contains(&"restored") => notices.push("restored"),
        _ => {}
    }
    if was_pending {
        notices.push("account_approved");
    }
    for kind in notices {
        crate::community::notifications::send(&state, by, &[id], kind, &serde_json::json!({ "url": "/settings" }))
            .await;
    }
    // Restricting an account owes its holder a statement of reasons.
    for (restricted, action) in [
        (matches!(p.suspended_until, Some(Some(_))), "suspended"),
        (matches!(p.silenced_until, Some(Some(_))), "silenced"),
    ] {
        if restricted {
            let facts = p
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .unwrap_or(en("Staff restricted the account for conduct against the site's terms and guidelines."));
            crate::moderation::dsa::record(
                &state,
                crate::moderation::dsa::Restriction {
                    user_id: id,
                    target_type: "account",
                    target_id: id,
                    action,
                    ground: "terms",
                    category: "scope_of_platform_service",
                    facts,
                    automated_detection: false,
                    automated_decision: false,
                    notice_id: None,
                    moderator_id: Some(by).filter(|b| *b > 0),
                },
            )
            .await;
        }
    }
    if let Some(level) = p.trust_level {
        crate::community::notifications::trust_raised(&state, id, level).await;
        crate::platform::webhooks::fire(
            &state,
            "trust.changed",
            serde_json::json!({ "user_id": id, "to": level, "by": "admin" }),
        );
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
