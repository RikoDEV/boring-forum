//! Language, text size and density.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct InterfaceForm {
    pub(super) csrf: String,
    pub(super) locale: String,
    pub(super) text_size: i16,
    pub(super) density: String,
    #[serde(default)]
    pub(super) date_format: String,
    pub(super) skin_tone: Option<i16>,
    #[serde(default)]
    pub(super) profile_visibility: String,
}

/// Language, text size (90–125 %) and density.
pub(super) async fn save_interface(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<InterfaceForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if !bf_core::i18n::supported(&f.locale)
        || !(90..=125).contains(&f.text_size)
        || !matches!(f.density.as_str(), "comfortable" | "compact")
    {
        return Err(AppError::Validation("Pick a listed language, a text size from 90 to 125 and a density.".into()));
    }
    let date_format = Some(f.date_format.as_str()).filter(|d| matches!(*d, "relative" | "iso" | "locale"));
    let visibility = Some(f.profile_visibility.as_str()).filter(|d| matches!(*d, "everyone" | "members" | "nobody"));
    let skin = f.skin_tone.filter(|s| (0..=5).contains(s));
    bf_db::accounts::preferences::save_interface(
        state.db()?,
        user.id,
        &f.locale,
        f.text_size,
        &f.density,
        date_format,
        skin,
        visibility,
    )
    .await
    .db()?;
    Ok(Redirect::to("/settings").into_response())
}

/// `/language/{code}` for visitors: remembers the choice in a cookie.
pub(super) async fn set_language(
    headers: axum::http::HeaderMap,
    Path(code): Path<String>,
) -> Result<Response, AppError> {
    if !bf_core::i18n::supported(&code) || crate::http::viewer::is_subresource(&headers) {
        return Err(AppError::NotFound);
    }
    let mut res = Redirect::to("/").into_response();
    let c = format!("bf_lang={code}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000");
    res.headers_mut().append(header::SET_COOKIE, HeaderValue::from_str(&c).map_err(AppError::internal)?);
    Ok(res)
}
