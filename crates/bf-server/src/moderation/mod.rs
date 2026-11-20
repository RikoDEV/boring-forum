//! Moderation: the review queue, post and topic tools, watched words, warnings, bans, the staff
//! panel, saved actions, the EU DSA and upload scanning.

use crate::state::AppState;
use axum::Router;

pub mod actions;
pub mod bans;
pub mod discipline;
pub mod dsa;
pub mod gate;
pub mod held;
pub mod panel;
pub mod posts;
pub mod report_reasons;
pub mod review;
pub mod scan;
pub mod spam;
pub mod topics;
pub mod warnings;
pub mod words;

/// Writes a staff action to the audit log. Never fatal: the action has already happened.
/// `actor` is `None` for the admin token. Put the state before and after in `details`.
pub(crate) async fn audit(
    state: &AppState,
    actor: Option<i64>,
    action: &str,
    target: String,
    details: serde_json::Value,
) {
    let Some(pool) = &state.pool else { return };
    if let Err(e) =
        bf_db::admin::audit::record(pool, bf_db::admin::audit::Entry::new(action).by(actor).on(target).details(details))
            .await
    {
        tracing::error!(error = %e, action, "audit entry not written");
    }
}

/// Every route of this part of the site.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(actions::router())
        .merge(bans::router())
        .merge(discipline::router())
        .merge(dsa::router())
        .merge(held::router())
        .merge(panel::router())
        .merge(posts::router())
        .merge(report_reasons::router())
        .merge(review::router())
        .merge(topics::router())
        .merge(warnings::router())
        .merge(words::router())
}
