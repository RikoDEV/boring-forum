//! Colour schemes: the palettes admins edit (ACP → Colour schemes), the site's default one, and
//! the stylesheet they are built into.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::admin::{schemes, themes};
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_SCHEMES: usize = 30;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/scheme/{file}", get(stylesheet))
        .route("/api/v1/admin/color-schemes", get(list).post(create))
        .route("/api/v1/admin/color-schemes/default", put(set_default))
        .route("/api/v1/admin/color-schemes/{id}", put(update).delete(remove))
}

/// Loads the schemes into the page renderer. Called with the themes, at start and after every change.
pub async fn install(state: &AppState) -> Result<(), AppError> {
    let rows = schemes::all(state.db()?).await.db()?;
    let list: Vec<_> = rows
        .iter()
        .map(|s| bf_web::schemes::Source {
            slug: &s.slug,
            name: &s.name,
            light: &s.light,
            dark: &s.dark,
            selectable: s.selectable,
        })
        .collect();
    let settings = state.settings();
    // A default that no longer exists falls back to the first scheme, so the site is never unstyled.
    let pick = |wanted: &'static str| {
        let slug = settings.str(wanted);
        if list.iter().any(|s| s.slug == slug) { slug } else { list.first().map_or("", |s| s.slug) }
    };
    bf_web::schemes::install(&list, pick("default_light_scheme"), pick("default_dark_scheme"));
    Ok(())
}

/// Hash-named, so it can be cached forever.
async fn stylesheet(Path(file): Path<String>) -> Result<Response, AppError> {
    let css = bf_web::schemes::stylesheet(&format!("/scheme/{file}")).ok_or(AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        css,
    )
        .into_response())
}

async fn list(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    let rows = schemes::all(state.db()?).await.db()?;
    let (_, light, dark) = bf_web::schemes::choices();
    Ok(Json(json!({
        "schemes": rows,
        "default_light": light,
        "default_dark": dark,
        "base": bf_web::schemes::BASE.iter().map(|(key, token, label)| json!({ "key": key, "token": token, "label": label })).collect::<Vec<_>>(),
        "derived": bf_web::schemes::DERIVED.iter().map(|(token, formula)| json!({ "token": token, "formula": formula })).collect::<Vec<_>>(),
    }))
    .into_response())
}

fn clean_name(name: &str) -> Result<String, AppError> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 40 {
        return Err(AppError::Validation("A colour scheme needs a name of up to 40 characters.".into()));
    }
    Ok(name)
}

/// The address name for a scheme's name: lower-case letters, digits and dashes.
fn slugify(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_matches('-');
    if s.is_empty() { "scheme".to_owned() } else { s.chars().take(34).collect() }
}

#[derive(Deserialize)]
struct NewScheme {
    name: String,
    /// The scheme whose colours to start from.
    copy_from: Option<i64>,
}

async fn create(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<NewScheme>,
) -> Result<Response, AppError> {
    let name = clean_name(&b.name)?;
    let pool = state.db()?;
    let rows = schemes::all(pool).await.db()?;
    if rows.len() >= MAX_SCHEMES {
        return Err(AppError::Validation("There are as many colour schemes as there can be.".into()));
    }
    let from = rows.iter().find(|s| Some(s.id) == b.copy_from).or(rows.first()).ok_or(AppError::NotFound)?;
    let base = slugify(&name);
    let mut slug = base.clone();
    let mut n = 2;
    while schemes::slug_taken(pool, &slug).await.db()? {
        slug = format!("{base}-{n}");
        n += 1;
    }
    let made = schemes::insert(pool, &slug, &name, &from.light, &from.dark).await.db()?;
    let details = json!({ "slug": slug, "name": name, "copy_from": from.id });
    crate::admin::api::audit_log(pool, who, "color_scheme.create", &format!("color_scheme:{}", made.id), details)
        .await?;
    changed(&state).await?;
    Ok((StatusCode::CREATED, Json(made)).into_response())
}

#[derive(Deserialize)]
struct Edit {
    name: String,
    light: Value,
    dark: Value,
    selectable: bool,
}

async fn update(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<Edit>,
) -> Result<Response, AppError> {
    let name = clean_name(&b.name)?;
    let light = bf_web::schemes::clean_palette(&b.light).map_err(AppError::Validation)?;
    let dark = bf_web::schemes::clean_palette(&b.dark).map_err(AppError::Validation)?;
    let (_, light_default, dark_default) = bf_web::schemes::choices();
    let current = schemes::all(state.db()?).await.db()?;
    let this = current.iter().find(|s| s.id == id).ok_or(AppError::NotFound)?;
    // The site's own schemes are always shown to members, whatever their switch says.
    let selectable = b.selectable || this.slug == light_default || this.slug == dark_default;
    schemes::update(state.db()?, id, &name, &light, &dark, selectable).await.found()?;
    let details = json!({ "name": name, "light": light, "dark": dark, "selectable": selectable });
    crate::admin::api::audit_log(state.db()?, who, "color_scheme.update", &format!("color_scheme:{id}"), details)
        .await?;
    changed(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let rows = schemes::all(state.db()?).await.db()?;
    let this = rows.iter().find(|s| s.id == id).ok_or(AppError::NotFound)?;
    let (_, light, dark) = bf_web::schemes::choices();
    if this.slug == light || this.slug == dark {
        return Err(AppError::Validation(
            "The site's default colour scheme cannot be deleted: choose another default first.".into(),
        ));
    }
    if themes::all(state.db()?)
        .await
        .db()?
        .iter()
        .any(|t| t.light_scheme.as_deref() == Some(&this.slug) || t.dark_scheme.as_deref() == Some(&this.slug))
    {
        return Err(AppError::Validation("A theme uses this colour scheme: choose another for it first.".into()));
    }
    schemes::delete(state.db()?, id).await.db()?;
    let details = json!({ "slug": this.slug, "name": this.name });
    crate::admin::api::audit_log(state.db()?, who, "color_scheme.delete", &format!("color_scheme:{id}"), details)
        .await?;
    changed(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct DefaultScheme {
    id: i64,
    /// Show its light palette to everyone who has not picked a scheme.
    #[serde(default)]
    light: bool,
    /// Show its dark palette likewise.
    #[serde(default)]
    dark: bool,
}

async fn set_default(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<DefaultScheme>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let rows = schemes::all(pool).await.db()?;
    let slug = rows.iter().find(|s| s.id == b.id).map(|s| s.slug.clone()).ok_or(AppError::NotFound)?;
    let value = Value::String(slug);
    let mut next = (*state.settings()).clone();
    for (key, on) in [("default_light_scheme", b.light), ("default_dark_scheme", b.dark)] {
        if on {
            next.set(key, value.clone()).map_err(|e| AppError::Validation(e.to_string()))?;
            bf_db::settings::save(pool, key, &value).await.map_err(AppError::internal)?;
            crate::admin::settings::bf_db_audit(pool, who, key, &value).await?;
        }
    }
    state.replace_settings(next);
    changed(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// After any change: rebuild the stylesheet and the themes (which carry the schemes they chose),
/// and let pages cached with the old ones go.
async fn changed(state: &AppState) -> Result<(), AppError> {
    crate::admin::themes::reload(state).await?;
    state.microcache.clear();
    Ok(())
}
