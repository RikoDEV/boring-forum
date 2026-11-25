//! Helpers every page handler uses: the database pool, error mapping, layouts and responses.

use crate::http::viewer::Viewer;
use crate::http::viewer::csrf_for;
use crate::http::viewer::is_staff;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::slug::profile_path;
use bf_web::pages::Layout;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

pub(crate) fn pool(state: &AppState) -> Result<&PgPool, AppError> {
    state.pool.as_ref().ok_or_else(|| AppError::internal("no database"))
}

pub(crate) fn db_err(e: sqlx::Error) -> AppError {
    AppError::internal(e)
}

/// `.db()` turns a database failure into the internal error page (`db_err`).
pub(crate) trait DbResult<T> {
    fn db(self) -> Result<T, AppError>;
    /// A write that reports whether a row matched: `false` is the 404 page.
    fn found(self) -> Result<(), AppError>
    where
        Self: Sized,
        T: Hit,
    {
        if self.db()?.hit() { Ok(()) } else { Err(AppError::NotFound) }
    }
}

/// What `found` asks of a write's result: a flag, or the count of rows it changed.
pub(crate) trait Hit {
    fn hit(&self) -> bool;
}

impl Hit for bool {
    fn hit(&self) -> bool {
        *self
    }
}

impl Hit for sqlx::postgres::PgQueryResult {
    fn hit(&self) -> bool {
        self.rows_affected() > 0
    }
}

impl<T> DbResult<T> for Result<T, sqlx::Error> {
    fn db(self) -> Result<T, AppError> {
        self.map_err(db_err)
    }
}

pub(crate) fn ago(t: DateTime<Utc>) -> bf_web::ago::Ago {
    bf_web::ago::Ago::new(t)
}

/// Builds the layout for this viewer. Members get a CSRF token, and a fresh cookie
/// when they have none; the returned header must be added to the response.
pub(crate) fn layout(state: &AppState, v: &Viewer, title: Option<&str>) -> (Layout, Option<HeaderValue>) {
    let mut l = Layout::new(&state.settings(), title);
    l.t = bf_core::i18n::T::new(v.locale);
    l.push_enabled = crate::platform::push::configured();
    l.set_nav(&state.settings(), &bf_core::ids::raw(&v.actor.groups), v.actor.is_admin);
    l.add_links(&bf_core::ids::raw(&v.actor.groups), v.actor.is_admin);
    crate::admin::themes::apply(&mut l, v, &v.theme_headers, None);
    // Link previews need a full address; the other pictures are fine as paths.
    if !l.brand.og_image.is_empty() {
        l.brand.og_image = format!("{}{}", state.config.base_url.trim_end_matches('/'), l.brand.og_image);
    }
    if let Some(u) = &v.user {
        l.unread_notifications = u.unread_notifications;
        l.unread_messages = u.unread_messages;
        l.unread_chat = u.unread_chat;
        l.admin = v.actor.is_admin;
        l.moderator = is_staff(v) || v.review_count.is_some();
        l.staff = is_staff(v);
        l.review_count = v.review_count.unwrap_or(0);
        l.user_url = profile_path(u.id.get(), &u.username);
        l.text_size = u.text_size;
        l.density.clone_from(&u.density);
        l.infinite = u.infinite_scroll;
        l.impersonating = v.impersonating;
        l.legal_pending = u.legal_pending;
        l.welcome = u.welcome_open;
        l.date_format.clone_from(&u.date_format);
        l.skin_tone = u.skin_tone;
    }
    l.user = v.user.as_ref().map(|u| u.username.clone());
    l.ad_groups = if v.actor.groups.is_empty() { vec![1] } else { bf_core::ids::raw(&v.actor.groups) };
    let (token, set) = csrf_for(v, v.user.is_some());
    l.csrf = token;
    (l, set)
}

pub(crate) fn page(body: Response, set_cookie: Option<HeaderValue>, cacheable: bool) -> Response {
    let mut res = body;
    if let Some(c) = set_cookie {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
    let cc = if cacheable { "public, s-maxage=60, stale-while-revalidate=600" } else { "private, no-store" };
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
    res
}

pub(crate) async fn unread_count(state: &AppState, v: &Viewer) -> i64 {
    crate::community::notifications::visible_unread(state, v).await
}

pub(crate) fn xml(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/xml; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=3600")], body)
        .into_response()
}
