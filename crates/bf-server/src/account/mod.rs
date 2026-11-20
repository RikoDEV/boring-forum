//! A member's own account: settings and the account life cycle.

use crate::state::AppState;
use axum::Router;

pub mod lifecycle;
pub mod settings;

/// Every route of this part of the site.
pub fn router() -> Router<AppState> {
    Router::new().merge(lifecycle::router()).merge(settings::router())
}
