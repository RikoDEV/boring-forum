//! The forum: index, topics, posting, lists, tags, search, feeds, reactions and topic tools.

use crate::state::AppState;
use axum::Router;

pub mod answers;
pub mod api;
pub mod assignment;
pub mod bookmarks;
pub mod embed;
pub mod feeds;
pub mod imgproxy;
pub mod index;
pub mod lists;
pub mod live;
pub mod live_topics;
pub mod metadata;
pub mod nofollow;
pub mod onebox;
pub mod posting;
pub mod reactions;
pub mod search;
pub mod summary;
pub mod tags;
pub mod topic_tools;
pub mod topics;
pub mod translations;
pub mod tree;
pub mod upload_access;
pub mod upload_links;
pub mod uploads;
pub mod views;
pub mod votes;

/// Every route of this part of the site.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(answers::router())
        .merge(api::router())
        .merge(assignment::router())
        .merge(bookmarks::router())
        .merge(embed::router())
        .merge(index::router())
        .merge(lists::router())
        .merge(live::router())
        .merge(live_topics::router())
        .merge(translations::router())
        .merge(onebox::router())
        .merge(posting::router())
        .merge(reactions::router())
        .merge(search::router())
        .merge(summary::router())
        .merge(tags::router())
        .merge(topic_tools::router())
        .merge(topics::router())
        .merge(uploads::router())
        .merge(votes::router())
}

/// Guest pages that may be served from the microcache.
pub fn cached_pages() -> Router<AppState> {
    Router::new()
        .merge(feeds::cached_pages())
        .merge(index::cached_pages())
        .merge(tags::cached_pages())
        .merge(topics::cached_pages())
}
