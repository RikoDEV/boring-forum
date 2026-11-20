//! Notification preferences, quiet hours and do-not-disturb.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Form;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_db::accounts::preferences;
use bf_db::community::notifications;
use serde::Deserialize;

/// Checkbox names: `{kind}_app`, `{kind}_email`, `{kind}_push` for each notification kind.
pub(super) async fn save_notifications(
    State(state): State<AppState>,
    v: Viewer,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let csrf = fields.iter().find(|(k, _)| k == "csrf").map(|(_, v)| v.as_str()).unwrap_or_default();
    v.check_csrf(csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let digest = fields.iter().find(|(k, _)| k == "digest").map_or("none", |(_, v)| v.as_str());
    if matches!(digest, "none" | "daily" | "weekly" | "monthly") {
        bf_db::platform::digests::set_digest(pool, user.id.get(), digest).await.db()?;
    }
    if crate::platform::newsletter::enabled(&state) {
        let on = fields.iter().any(|(k, v)| k == "newsletter" && v == "1");
        bf_db::platform::newsletter::set(pool, user.id.get(), on).await.db()?;
    }
    if state.settings().bool("mailing_list_mode") {
        let on = fields.iter().any(|(k, v)| k == "mailing_list" && v == "1");
        bf_db::community::notifications::set_mailing_list(pool, user.id.get(), on).await.db()?;
    }
    for kind in crate::account::settings::switchable().map(|k| k.key) {
        let on = |ch: &str| fields.iter().any(|(k, _)| *k == format!("{kind}_{ch}"));
        let channels = if on("app") { preferences::APP } else { 0 }
            | if on("email") { preferences::EMAIL } else { 0 }
            | if on("push") { preferences::PUSH } else { 0 };
        notifications::set_notification_pref(pool, user.id.get(), kind, channels).await.db()?;
    }
    Ok(Redirect::to("/settings").into_response())
}

/// What members can switch on and off. Notices about their own account are always sent.
pub(crate) fn switchable() -> impl Iterator<Item = &'static bf_core::notifications::Kind> {
    bf_core::notifications::all().into_iter().filter(|k| !k.mandatory)
}

pub(crate) async fn prefs_view(
    pool: &sqlx::PgPool,
    user_id: i64,
) -> Result<Vec<(String, String, bool, bool, bool)>, AppError> {
    let saved = notifications::notification_prefs(pool, user_id).await.db()?;
    Ok(switchable()
        .map(|k| {
            let ch =
                saved.iter().find(|(sk, _)| sk == k.key).map_or(notifications::default_channels(k.key), |(_, c)| *c);
            (
                k.key.to_owned(),
                k.label.to_owned(),
                ch & preferences::APP != 0,
                ch & preferences::EMAIL != 0,
                ch & preferences::PUSH != 0,
            )
        })
        .collect())
}

#[derive(Deserialize)]
pub(super) struct DndForm {
    pub(super) csrf: String,
    pub(super) hours: i32,
}

#[derive(Deserialize)]
pub(super) struct QuietForm {
    pub(super) csrf: String,
    #[serde(default)]
    pub(super) start: String,
    #[serde(default)]
    pub(super) end: String,
}

/// A daily window without email or push. Leaving either hour empty turns it off.
pub(super) async fn set_quiet(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<QuietForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let hour = |s: &str| -> Result<Option<i16>, AppError> {
        match s.trim() {
            "" => Ok(None),
            h => h
                .parse::<i16>()
                .ok()
                .filter(|h| (0..=23).contains(h))
                .map(Some)
                .ok_or_else(|| AppError::Validation("Hours run from 0 to 23.".into())),
        }
    };
    let (start, end) = (hour(&f.start)?, hour(&f.end)?);
    let (start, end) = if start.is_some() && end.is_some() { (start, end) } else { (None, None) };
    bf_db::accounts::preferences::set_quiet_hours(state.db()?, user.id.get(), start, end).await.db()?;
    Ok(Redirect::to("/settings#s-notifications").into_response())
}

pub(super) async fn set_dnd(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<DndForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if !(0..=8760).contains(&f.hours) {
        return Err(AppError::Validation("Choose up to a year.".into()));
    }
    preferences::set_dnd(state.db()?, user.id.get(), f.hours).await.db()?;
    Ok(Redirect::to("/settings").into_response())
}
