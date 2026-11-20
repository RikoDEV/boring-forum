//! Announcements and the site banner.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_core::perms::{Actor, Perms};
use bf_core::slug::topic_path;
use bf_db::admin::announcements;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/t/{id}/banner", post(banner))
        .route("/api/v1/admin/announcements", get(list_announcements).post(create_announcement))
        .route("/api/v1/admin/announcements/{id}", put(update_announcement).delete(delete_announcement))
}

/// Keeps announcements in memory, so showing a banner costs pages no query.
pub(crate) async fn load_announcements(state: &AppState) -> Result<(), AppError> {
    load_announcements_local(state).await?;
    crate::platform::cluster::notify("announcements");
    Ok(())
}

/// This node only: another node already told the rest.
pub(crate) async fn load_announcements_local(state: &AppState) -> Result<(), AppError> {
    let rows = announcements::announcements(state.db()?).await.db()?;
    bf_web::theme::set_announcements(
        rows.into_iter()
            .filter(|a| a.enabled)
            .map(|a| bf_web::theme::Announcement {
                id: a.id,
                title: a.title,
                body: a.body,
                html: a.body_html,
                starts_at: a.starts_at,
                ends_at: a.ends_at,
            })
            .collect(),
    );
    state.microcache.clear();
    Ok(())
}

async fn list_announcements(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    Ok(Json(announcements::announcements(state.db()?).await.db()?).into_response())
}

fn cook_announcement(a: &mut announcements::Announcement) -> Result<(), AppError> {
    if a.title.trim().is_empty() || a.title.chars().count() > 150 || a.body.chars().count() > 5000 {
        return Err(AppError::Validation("An announcement needs a title and at most 5000 characters.".into()));
    }
    // Always from the source, never from what the request says the HTML is.
    a.body_html = bf_cook::cook(&a.body).html;
    Ok(())
}

/// One line of the staff log for an announcement: shown on every page, so who wrote it is kept.
async fn audit(
    state: &AppState,
    who: Option<i64>,
    action: &str,
    id: i64,
    a: Option<&announcements::Announcement>,
) -> Result<(), AppError> {
    let details = match a {
        Some(a) => json!({
            "title": a.title, "body": a.body, "enabled": a.enabled, "starts_at": a.starts_at, "ends_at": a.ends_at,
        }),
        None => json!({}),
    };
    crate::admin::api::audit_log(state.db()?, who, action, &format!("announcement:{id}"), details).await
}

async fn create_announcement(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(mut a): Json<announcements::Announcement>,
) -> Result<Response, AppError> {
    cook_announcement(&mut a)?;
    let id = announcements::save_announcement(state.db()?, 0, &a).await.db()?;
    audit(&state, who, "announcement.create", id.unwrap_or(0), Some(&a)).await?;
    load_announcements(&state).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update_announcement(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(mut a): Json<announcements::Announcement>,
) -> Result<Response, AppError> {
    cook_announcement(&mut a)?;
    if id <= 0 || announcements::save_announcement(state.db()?, id, &a).await.db()?.is_none() {
        return Err(AppError::NotFound);
    }
    audit(&state, who, "announcement.update", id, Some(&a)).await?;
    load_announcements(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_announcement(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    announcements::delete_announcement(state.db()?, id).await.found()?;
    audit(&state, who, "announcement.delete", id, None).await?;
    load_announcements(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Links a topic from a site-wide announcement. Only for topics a guest can read: the banner
/// shows its title to everyone.
async fn banner(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let (t, tree, _) = readable_topic(&state, &v, id).await?;
    if !crate::http::viewer::is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let guest = Actor { groups: vec![1.into()], ..Default::default() };
    if !tree.perms(&guest, t.node_id).has(Perms::READ) {
        return Err(AppError::Validation("Only a public topic can be a banner.".into()));
    }
    let url = topic_path(t.id, &t.slug);
    let body = format!("[Read the topic]({url})");
    let a = bf_db::admin::announcements::Announcement {
        id: 0,
        title: t.title.clone(),
        body_html: bf_cook::cook(&body).html,
        body,
        enabled: true,
        starts_at: None,
        ends_at: None,
    };
    let made = bf_db::admin::announcements::save_announcement(state.db()?, 0, &a).await.db()?;
    audit(&state, v.user.as_ref().map(|u| u.id.get()), "announcement.create", made.unwrap_or(0), Some(&a)).await?;
    crate::admin::announcements::load_announcements(&state).await?;
    Ok(Redirect::to(&url).into_response())
}
