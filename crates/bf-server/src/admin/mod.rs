//! Administration: the admin API used by the SPA, settings, dashboard, maintenance, themes,
//! pages, announcements, invites, emoji, mail templates and text overrides.

use crate::state::AppState;
use axum::Router;

pub mod announcements;
pub mod api;
pub mod backups;
pub mod bulk_mail;
pub mod dashboard;
pub mod emoji;
pub mod explorer;
pub mod forums;
pub mod invites;
pub mod mail_preview;
pub mod mail_templates;
pub mod maintenance;
pub mod pages;
pub mod permalinks;
pub mod reports;
pub mod schemes;
pub mod settings;
pub mod suggestions;
pub mod text_overrides;
pub mod themes;

/// Every route of this part of the site.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(announcements::router())
        .merge(api::router())
        .merge(backups::router())
        .merge(bulk_mail::router())
        .merge(dashboard::router())
        .merge(suggestions::router())
        .merge(emoji::router())
        .merge(explorer::router())
        .merge(forums::router())
        .merge(invites::router())
        .merge(mail_preview::router())
        .merge(maintenance::router())
        .merge(pages::router())
        .merge(reports::router())
        .merge(text_overrides::router())
        .merge(permalinks::router())
        .merge(schemes::router())
        .merge(themes::router())
}
