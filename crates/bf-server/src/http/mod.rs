//! HTTP plumbing shared by every page: the request context, rendering helpers, middleware,
//! rate limiting, API semantics and observability.

use crate::state::AppState;
use axum::Router;

pub mod api;
pub mod i18n;
pub mod middleware;
pub mod observability;
pub mod ratelimit;
pub mod render;
pub mod traffic;
pub mod ui;
pub mod viewer;

/// Every route of this part of the site.
pub fn router() -> Router<AppState> {
    Router::new().merge(middleware::router()).merge(observability::router())
}
