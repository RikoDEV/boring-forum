//! The getting-started page.

use crate::http::render::{DbResult, layout, page};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Form;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_db::community::onboarding;
use bf_web::pages::{WelcomePage, WelcomeStep, render};

pub fn router() -> Router<AppState> {
    Router::new().route("/welcome", get(welcome)).route("/welcome/dismiss", post(dismiss))
}

#[derive(serde::Deserialize)]
struct Dismiss {
    csrf: String,
}

/// Hides the getting-started banner for this member for good, and returns to the page they were on.
async fn dismiss(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Form(f): Form<Dismiss>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    v.check_csrf(&f.csrf)?;
    onboarding::dismiss_welcome(state.db()?, user.id.get()).await.db()?;
    // Only a path on this site: the Referer's address part is dropped.
    let back = headers
        .get(header::REFERER)
        .and_then(|h| h.to_str().ok())
        .and_then(|r| r.split_once("://").map(|(_, rest)| rest))
        .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
        .filter(|p| p.starts_with('/') && !p.starts_with("//") && !p.starts_with("/\\"))
        .unwrap_or("/");
    Ok(Redirect::to(back).into_response())
}

/// Guided first steps, none required.
async fn welcome(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let (avatar, about, posted, verified) = onboarding::onboarding(state.db()?, user.id.get()).await.db()?;
    let profile = bf_core::slug::profile_path(user.id.get(), &user.username);
    let mut steps = vec![
        WelcomeStep {
            done: verified,
            next: false,
            icon: "mail",
            title: en("Confirm your email"),
            text: en("So you can recover your account and get the notifications you choose."),
            action: en("Confirm email"),
            url: "/settings#s-email".to_owned(),
        },
        WelcomeStep {
            done: avatar,
            next: false,
            icon: "camera",
            title: en("Add a picture"),
            text: en("A face or an image makes your posts easier to recognise."),
            action: en("Add a picture"),
            url: profile,
        },
        WelcomeStep {
            done: about,
            next: false,
            icon: "pencil",
            title: en("Say something about yourself"),
            text: en("A line or two on your profile helps people know who they are talking to."),
            action: en("Edit profile"),
            url: "/settings#s-profile".to_owned(),
        },
        WelcomeStep {
            done: posted,
            next: false,
            icon: "message-square",
            title: en("Write your first post"),
            text: en("Reply to a topic or start your own. Everyone starts somewhere."),
            action: en("Go to the forums"),
            url: "/".to_owned(),
        },
    ];
    if let Some(s) = steps.iter_mut().find(|s| !s.done) {
        s.next = true;
    }
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Getting started")));
    l.noindex = true;
    // This page is the checklist: no banner pointing at itself.
    l.welcome = false;
    let body = WelcomePage {
        layout: l,
        name: user.username.clone(),
        done: steps.iter().filter(|s| s.done).count(),
        total: steps.len(),
        steps,
    };
    Ok(page(render(&body), set, false))
}
