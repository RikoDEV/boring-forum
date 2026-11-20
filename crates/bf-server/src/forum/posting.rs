//! Writing: new topics, replies, edits, deletion, previews, autocomplete and reply by email.

use crate::state::AppState;
use axum::Router;
use axum::routing::{get, post};

mod by_email;
mod compose;
mod edit;
mod new_topic;
mod notify;
mod replies;
mod rules;

use self::compose::{autocomplete, invite_to_topic, preview};
use self::edit::{change_author, post_raw, post_source};
pub(crate) use self::edit::{delete, edit};
use self::new_topic::{new_topic, new_topic_form, pin_options};
use self::notify::after_post;
use self::replies::{BodyForm, reply};
use self::rules::{
    Activity, check_activity_limits, check_content_rules, check_duplicate_title, check_image_limit,
    check_new_member_links, daily_limit, screen, validate_body,
};

pub(crate) use self::by_email::{reply_by_email, topic_by_email};
pub(crate) use self::edit::editable_post;
pub(crate) use self::new_topic::{apply_tags, clean_tag, create_targets};
pub(crate) use self::notify::{after_plugin_post, after_system_post, announce_post};
pub(crate) use self::rules::{check_federated_cooked, screen_federated, validate_title};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/c/{id}/new", get(new_topic_form).post(new_topic))
        .route("/t/{id}/reply", post(reply))
        .route("/t/{id}/pin", post(pin_options))
        .route("/post/{id}/edit", post(edit))
        .route("/api/v1/posts/{id}/source", get(post_source))
        .route("/post/{id}/delete", post(delete))
        .route("/api/v1/autocomplete", get(autocomplete))
        .route("/t/{id}/author", post(change_author))
        .route("/api/v1/posts/{id}/raw", get(post_raw))
        .route("/api/v1/preview", post(preview))
        .route("/t/{id}/invite", post(invite_to_topic))
}
