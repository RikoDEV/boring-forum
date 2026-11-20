//! Groups: create, restyle, delete and their limits.

use crate::admin::api::{Admin, AdminWrite};
use crate::http::render::{DbResult, db_err};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_core::slug::slugify;
use bf_db::admin::members;
use serde::Deserialize;
use serde_json::json;

use super::actor;

admin_get!(groups(state) members::groups(state.db()?).await.db()?);

#[derive(Deserialize)]
pub(super) struct NewGroup {
    pub(super) name: String,
}

pub(super) async fn create_group(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(g): Json<NewGroup>,
) -> Result<Response, AppError> {
    let name = g.name.trim();
    if name.is_empty() || name.chars().count() > 50 {
        return Err(AppError::Validation("Group names are 1–50 characters.".into()));
    }
    match members::create_group(state.db()?, name, &slugify(name), actor(who)).await {
        Ok(id) => Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            Err(AppError::Validation("That group already exists.".into()))
        }
        Err(e) => Err(db_err(e)),
    }
}

#[derive(Deserialize)]
pub(super) struct GroupBody {
    pub(super) name: String,
    #[serde(default)]
    pub(super) slug: String,
    #[serde(default)]
    pub(super) colour: String,
    #[serde(default)]
    pub(super) icon: String,
    #[serde(default)]
    pub(super) image: String,
}

/// Renames and restyles a group. Guests (1) keep their name: it is how the site refers to
/// visitors who are not signed in, and no one is ever shown in it.
pub(super) async fn update_group(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(g): Json<GroupBody>,
) -> Result<Response, AppError> {
    let bad = |m: &str| Err(AppError::Validation(m.into()));
    let name = g.name.trim();
    if name.is_empty() || name.chars().count() > 50 {
        return bad("Group names are 1–50 characters.");
    }
    // The slug is the group's address (/g/…) and how staff @mention it.
    let slug = if g.slug.trim().is_empty() { slugify(name) } else { slugify(g.slug.trim()) };
    if slug.is_empty() || slug.len() > 50 {
        return bad(en("Choose an address of letters, digits and dashes."));
    }
    let colour = g.colour.trim().to_lowercase();
    let colour = Some(colour).filter(|c| !c.is_empty());
    if colour
        .as_deref()
        .is_some_and(|c| c.len() != 7 || !c.starts_with('#') || !c[1..].bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return bad(en("Use a colour like #3b82f6."));
    }
    let icon = Some(g.icon.trim()).filter(|i| !i.is_empty());
    if icon
        .is_some_and(|i| i.len() > 40 || !i.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
    {
        return bad(en("Choose an icon from the list."));
    }
    let image = Some(g.image.trim()).filter(|i| !i.is_empty());
    if image.is_some_and(|i| !bf_core::settings::upload_path_ok(i)) {
        return bad(en("The badge picture must be an upload."));
    }
    let pool = state.db()?;
    let current = members::groups(pool).await.db()?.into_iter().find(|x| x.id == id).ok_or(AppError::NotFound)?;
    if id == 1 && (name != current.name || slug != current.slug) {
        return bad(en("The guests group cannot be renamed."));
    }
    if current.club.is_some() && (name != current.name || slug != current.slug) {
        return bad(en("This group belongs to a club; it is named after the club."));
    }
    let style = members::GroupStyle { name, slug: &slug, colour: colour.as_deref(), icon, image };
    match members::update_group(pool, id, &style, actor(who)).await {
        Ok(true) => {}
        Ok(false) => return Err(AppError::NotFound),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return bad(en("Another group already has that name or address."));
        }
        Err(e) => return Err(db_err(e)),
    }
    // Names and colours are shown on every page: posts, profiles and the member menu.
    state.sessions.invalidate_all();
    state.forum.invalidate();
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Deletes a group the admin made. The four built-in groups stay; club groups go with their club.
pub(super) async fn delete_group(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    if id <= 4 {
        return Err(AppError::Validation("The built-in groups cannot be deleted.".into()));
    }
    let pool = state.db()?;
    if let Some(why) = members::group_in_use(pool, id).await.db()? {
        return Err(AppError::Validation(why.into()));
    }
    members::delete_group(pool, id, actor(who)).await.found()?;
    // Members lose what the group gave them at once.
    state.sessions.invalidate_all();
    state.forum.invalidate();
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Body: `{"max_upload_kb": 1024, "posts_per_day": 20}`; omitted keys mean no limit.
pub(super) async fn set_group_limits(
    State(state): State<AppState>,
    Admin(who, _v): AdminWrite,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Map<String, serde_json::Value>>,
) -> Result<Response, AppError> {
    let mut limits = serde_json::Map::new();
    for (k, v) in body {
        let n = v
            .as_i64()
            .filter(|n| *n >= 0)
            .ok_or_else(|| AppError::invalid("{k} must be a whole number", &[("k", &k)]))?;
        if !matches!(
            k.as_str(),
            "max_upload_kb"
                | "posts_per_day"
                | "edit_minutes"
                | "max_images_per_post"
                | "pm_per_hour"
                | "pm_max_participants"
        ) {
            return Err(AppError::invalid("Unknown limit {k}", &[("k", &k)]));
        }
        limits.insert(k, json!(n));
    }
    bf_db::accounts::limits::set_group_limits(state.db()?, id, &serde_json::Value::Object(limits), actor(who))
        .await
        .found()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
