//! Members together: profiles, follows, notifications, messages, chat, clubs, directory,
//! statuses, badges, polls and events.

use crate::state::AppState;
use axum::Router;

pub mod avatars;
pub mod badges;
pub mod browsing;
pub mod chat;
pub mod clubs;
pub mod contact;
pub mod directory;
pub mod events;
pub mod follows;
pub mod groups;
pub mod legal;
pub mod messages;
pub mod notifications;
pub mod onboarding;
pub(crate) mod people;
pub mod polls;
pub mod presence;
pub mod profiles;
pub mod promotion;
pub mod referrals;
pub mod statuses;
pub mod trust;

/// Every route of this part of the site.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(avatars::router())
        .merge(badges::router())
        .merge(chat::router())
        .merge(clubs::router())
        .merge(contact::router())
        .merge(directory::router())
        .merge(events::router())
        .merge(follows::router())
        .merge(groups::router())
        .merge(legal::router())
        .merge(messages::router())
        .merge(notifications::router())
        .merge(onboarding::router())
        .merge(polls::router())
        .merge(presence::router())
        .merge(profiles::router())
        .merge(promotion::router())
        .merge(referrals::router())
        .merge(statuses::router())
        .merge(trust::router())
}

/// Guest pages that may be served from the microcache.
pub fn cached_pages() -> Router<AppState> {
    Router::new().merge(profiles::cached_pages())
}
