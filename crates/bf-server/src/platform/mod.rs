//! Integration and background work: webhooks, API keys, outbound HTTP, plugins, push, mail,
//! digests, the audit sink, the importer, OpenAPI and discovery files.

use crate::state::AppState;
use axum::Router;

pub mod ads;
pub mod api_keys;
pub mod audit_sink;
pub mod automation;
pub mod backup;
pub mod builtin_plugins;
pub mod cluster;
pub mod digest;
pub mod discovery;
pub mod imap_in;
pub mod importer;
pub mod indexnow;
pub mod mail;
pub mod newsletter;
pub mod og;
pub mod openapi;
mod openapi_docs;
pub mod outbound;
pub mod plugins;
pub mod push;
pub mod webhooks;

/// Every route of this part of the site.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(ads::router())
        .merge(api_keys::router())
        .merge(automation::router())
        .merge(discovery::router())
        .merge(newsletter::router())
        .merge(og::router())
        .merge(plugins::router())
        .merge(push::router())
        .merge(webhooks::router())
}
