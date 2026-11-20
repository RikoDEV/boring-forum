//! Profile fields and the signature.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Form;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_db::accounts::preferences;
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct ProfileForm {
    pub(super) csrf: String,
    #[serde(default)]
    pub(super) about: String,
    #[serde(default)]
    pub(super) location: String,
    #[serde(default)]
    pub(super) website: String,
    #[serde(default)]
    pub(super) timezone: String,
    #[serde(default)]
    pub(super) display_name: String,
    #[serde(default)]
    pub(super) hide_last_seen: String,
    #[serde(flatten)]
    pub(super) extra: std::collections::HashMap<String, String>,
}

pub(super) fn opt(s: &str, max: usize) -> Option<String> {
    let t = bf_core::html::strip_hidden_chars(s);
    let t = t.trim();
    (!t.is_empty()).then(|| t.chars().take(max).collect())
}

fn bad_website(w: &str) -> bool {
    !(w.starts_with("https://") || w.starts_with("http://")) || w.contains(char::is_whitespace)
}

pub(super) async fn save_profile(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<ProfileForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if !user.approved {
        return Err(AppError::Validation("Your account is waiting for approval by the staff.".into()));
    }
    // Signatures, statuses, avatars and chat all refuse a silenced member; so does the profile.
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    let website = opt(&f.website, 200);
    if website.as_deref().is_some_and(bad_website) {
        return Err(AppError::Validation("Website must start with https:// or http://".into()));
    }
    let timezone = opt(&f.timezone, 64);
    if timezone.as_deref().is_some_and(|z| !z.chars().all(|c| c.is_ascii_alphanumeric() || "/_+-".contains(c))) {
        return Err(AppError::Validation("Time zone must look like Europe/Warsaw".into()));
    }
    let pool = state.db()?;
    let current = preferences::profile_fields(pool, user.id).await.db()?;
    // Bio and website are where spam accounts put their links, so they pass the same gate as a
    // post: a ready account, watched words, and the new-member link limit (the website counts).
    // A text with a "require approval" word keeps its old value and waits in the held queue;
    // the rest of the form is saved now.
    let mut held = serde_json::Map::new();
    let mut keep = |key: &str, c: crate::moderation::gate::Checked, old: Option<String>| {
        if c.held {
            held.insert(key.to_owned(), serde_json::Value::String(c.text));
            old
        } else {
            Some(c.text)
        }
    };
    let mut about_links = 0;
    let about = match opt(&f.about, 5000) {
        Some(a) => {
            let (a, cooked) = crate::moderation::gate::check(&state, &v, &a).await?;
            about_links = cooked.links.len();
            keep("about", a, current.about_md.clone())
        }
        None => None,
    };
    if website.is_some() {
        crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    }
    // The address meets the watched words like the bio does: blocked words refuse it, a
    // "require approval" word keeps the old one until a moderator decides.
    let website = match website {
        Some(w) => {
            let w = keep("website", crate::moderation::gate::words(&state, &v, &w).await?, current.website.clone());
            if w.as_deref().is_some_and(bad_website) {
                return Err(AppError::Validation("Website must start with https:// or http://".into()));
            }
            w
        }
        None => None,
    };
    crate::moderation::gate::links(&state, &v, user, about_links + usize::from(website.is_some()))?;
    let display_name = match opt(&f.display_name, 40) {
        Some(n) => {
            // A display name that passes for staff or a reserved name is how members get trusted
            // they should not be; it is checked as a username's look-alike is.
            if !crate::http::viewer::is_staff(&v)
                && bf_db::accounts::users::display_name_impersonates(pool, &n, Some(user.id.get())).await.db()?
            {
                return Err(AppError::Validation(
                    "That display name looks too much like a staff member's name.".into(),
                ));
            }
            keep("display_name", crate::moderation::gate::words(&state, &v, &n).await?, current.display_name.clone())
        }
        None => None,
    };
    let location = match opt(&f.location, 100) {
        Some(l) => keep("location", crate::moderation::gate::words(&state, &v, &l).await?, current.location.clone()),
        None => None,
    };
    let html = about.as_deref().map(|a| bf_cook::cook(a).html);
    let fields = preferences::ProfileFields {
        display_name,
        hide_last_seen: f.hide_last_seen == "1",
        profile_visibility: String::new(), // saved with the interface settings
        about_md: about,
        location,
        website,
        timezone,
    };
    let mut answers = Vec::new();
    for field in bf_db::accounts::fields::fields(pool).await.db()? {
        // A plugin sets the answers of the fields it keeps to itself.
        if !field.editable {
            continue;
        }
        let raw = f.extra.get(&format!("field_{}", field.id)).map_or("", String::as_str);
        answers.push((field.id, field.check_in(&v.t(), raw).map_err(AppError::Validation)?));
    }
    preferences::save_profile(pool, user.id, &fields, html.as_deref()).await.db()?;
    bf_db::accounts::fields::save_values(pool, user.id, &answers).await.db()?;
    crate::platform::webhooks::fire(
        &state,
        "profile.updated",
        serde_json::json!({ "user_id": user.id, "username": user.username }),
    );
    state.microcache.clear();
    if !held.is_empty() {
        crate::moderation::held::hold(&state, user.id.get(), "profile", None, serde_json::Value::Object(held)).await?;
        return Ok(crate::moderation::held::held_page(&state, &v));
    }
    Ok(Redirect::to("/settings").into_response())
}

#[derive(Deserialize)]
pub(super) struct SignatureForm {
    pub(super) csrf: String,
    #[serde(default)]
    pub(super) signature: String,
}

/// Cooked, then held to text and links: images could otherwise fill every post a member makes.
pub(super) async fn save_signature(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<SignatureForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    if f.signature.chars().count() as i64 > state.settings().int("signature_max_length") {
        return Err(AppError::Validation("That signature is too long.".into()));
    }
    let (signature, cooked) = crate::moderation::gate::check(&state, &v, f.signature.trim()).await?;
    if cooked.html.contains("<img") {
        return Err(AppError::Validation("Signatures cannot contain images.".into()));
    }
    if signature.held {
        let payload = serde_json::json!({ "signature": signature.text.trim() });
        crate::moderation::held::hold(&state, user.id.get(), "signature", None, payload).await?;
        return Ok(crate::moderation::held::held_page(&state, &v));
    }
    preferences::set_signature(state.db()?, user.id.get(), signature.text.trim(), &cooked.html).await.db()?;
    state.microcache.clear();
    Ok(Redirect::to("/settings").into_response())
}
