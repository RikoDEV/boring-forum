//! Replying to a topic.

use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::moderation::review::awaiting_approval;
use crate::state::AppState;
use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::forum::{links, posts};
use serde::Deserialize;

use super::{
    Activity, after_post, after_system_post, check_activity_limits, check_content_rules, check_image_limit,
    check_new_member_links, daily_limit, screen, validate_body,
};

#[derive(Deserialize)]
pub(crate) struct BodyForm {
    #[serde(default)]
    pub(super) anonymous: String,
    pub(super) csrf: String,
    pub(super) body: String,
    /// Post number this reply answers.
    #[serde(default)]
    pub(super) reply_to: String,
    /// Edit count when the edit form was loaded (concurrent edit check).
    #[serde(default)]
    pub(super) base: String,
    #[serde(default)]
    pub(super) reason: String,
}

pub(super) async fn reply(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<BodyForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    let archived_forum = state.forum.get(state.db()?).await?.node(t.node_id).is_some_and(|n| n.kind == "archive");
    if !perms.has(Perms::REPLY) || ((t.locked || t.archived || archived_forum) && !perms.has(Perms::MODERATE)) {
        return Err(AppError::Forbidden);
    }
    validate_body(&state, &f.body).map_err(AppError::Validation)?;
    crate::forum::topic_tools::check_slow_mode(&state, id, user.id.get(), perms.has(Perms::MODERATE)).await?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    daily_limit(&state, user).await?;
    check_activity_limits(&state, user, Activity::Reply(id)).await?;
    let checked = screen(
        &state,
        &tree,
        user,
        &v.actor,
        t.node_id,
        &[&f.body],
        None,
        posts::approved_post_count(
            state.db()?,
            user.id.get(),
            state.settings().int("approve_first_posts"),
        )
        .await
        .db()?,
    )
    .map_err(AppError::Validation)?;
    super::rules::check_duplicate_post(&state, user, &checked.texts[0]).await?;
    let filtered = crate::platform::plugins::before_post(&state, user.id.get(), &checked.texts[0]).await?;
    // Replace rules and plugin filters can lengthen the text: the limit applies to what is stored.
    validate_body(&state, &filtered).map_err(AppError::Validation)?;
    let cooked = crate::platform::plugins::cook(&state, &filtered).await;
    check_new_member_links(&state, user, cooked.links.len())?;
    check_image_limit(&state, user, &cooked.html).await?;
    check_content_rules(&state, user, &filtered, &cooked.html)?;
    let post = posts::NewPost {
        author_id: user.id.get(),
        raw: &filtered,
        cooked: &cooked.html,
        cooked_version: bf_cook::COOK_VERSION,
        ip: v.ip,
        pending: checked.pending,
    };
    // Anonymous: the post is written under the shared account in the same transaction, so there is
    // no moment at which it carries the real name, and nothing after it may carry it either.
    let anonymous = f.anonymous == "1" && state.settings().bool("anonymous_posting");
    if anonymous {
        let (number, anon) = posts::create_anonymous_reply(pool, id, t.node_id, &post).await.db()?;
        links::record_links(pool, id, number, &cooked.links).await.db()?;
        if let Ok(to) = f.reply_to.parse::<i32>() {
            posts::set_reply_to(pool, id, number, to).await.db()?;
        }
        bf_db::forum::uploads::delete_draft(pool, user.id.get(), &format!("reply:{id}")).await.db()?;
        state.forum.invalidate();
        if checked.pending {
            crate::community::notifications::tell_reviewers(&state, t.node_id, user.id.get()).await;
            return Ok(awaiting_approval(&state, &v));
        }
        after_system_post(&state, anon, bf_core::i18n::en("Anonymous"), &t.title, &t.slug, id, t.node_id, number).await;
        return Ok(Redirect::to(&format!("{}#post-{number}", topic_path(id, &t.slug))).into_response());
    }
    let number = posts::create_reply(pool, id, t.node_id, &post).await.db()?;
    links::record_links(pool, id, number, &cooked.links).await.db()?;
    if let Ok(to) = f.reply_to.parse::<i32>() {
        posts::set_reply_to(pool, id, number, to).await.db()?;
    }
    bf_db::forum::uploads::delete_draft(pool, user.id.get(), &format!("reply:{id}")).await.db()?;
    state.forum.invalidate();
    if checked.pending {
        crate::community::notifications::tell_reviewers(&state, t.node_id, user.id.get()).await;
        return Ok(awaiting_approval(&state, &v));
    }
    after_post(&state, user, id, t.node_id, &t.title, &t.slug, number, &filtered, &cooked.mentions).await;
    let per_page = state.settings().int("posts_per_page") as i32;
    let page_n = (number - 1) / per_page + 1;
    let base = topic_path(id, &t.slug);
    let url = if page_n <= 1 { base } else { format!("{base}?page={page_n}") };
    Ok(Redirect::to(&format!("{url}#post-{number}")).into_response())
}
