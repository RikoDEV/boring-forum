//! Topics and replies that arrive by email, under the same rules as the site's forms.

use crate::http::render::DbResult;
use crate::state::AppState;
use bf_core::AppError;
use bf_core::perms::{Actor, Perms};
use bf_core::slug::slugify;
use bf_db::accounts::users;
use bf_db::forum::{links, posts, topics};

use super::new_topic::{apply_tags, check_tags};
use super::{
    Activity, after_post, check_activity_limits, check_content_rules, check_image_limit, check_new_member_links,
    daily_limit, screen, validate_body, validate_title,
};

/// A new topic that arrived by email, under the same rules as the site's form.
pub(crate) async fn topic_by_email(
    state: &AppState,
    user_id: i64,
    node: i64,
    subject: &str,
    body: &str,
) -> Result<(), AppError> {
    let pool = state.db()?;
    let user = users::user_by_id(pool, user_id.into()).await.db()?.ok_or(AppError::NotFound)?;
    // Everything a signed-in member would be stopped by on the site.
    if bf_db::accounts::users::is_suspended(pool, user.id.get()).await.db()? || !user.approved || user.legal_pending {
        return Err(AppError::Forbidden);
    }
    let actor = Actor {
        user_id: Some(user.id),
        groups: user.groups.clone(),
        is_admin: user.is_admin,
        silenced: user.silenced,
        moderated_nodes: user.moderated_nodes.clone(),
        denied: crate::platform::plugins::denials(state, user.id.get(), &user.groups).await,
    };
    let tree = state.forum.get(pool).await?;
    let found = tree.node(node).is_some_and(|n| n.kind == "forum");
    if !found || !tree.perms(&actor, node).has(Perms::CREATE) || actor.silenced {
        return Err(AppError::Forbidden);
    }
    let title = subject.trim();
    validate_title(state, title).map_err(AppError::Validation)?;
    validate_body(state, body).map_err(AppError::Validation)?;
    daily_limit(state, &user).await?;
    check_activity_limits(state, &user, Activity::Topic).await?;
    let checked = screen(
        state,
        &tree,
        &user,
        &actor,
        node,
        &[title, body],
        None,
        posts::approved_post_count(state.db()?, user.id.get(), state.settings().int("approve_first_posts"))
            .await
            .db()?,
    )
    .map_err(AppError::Validation)?;
    let filtered = crate::platform::plugins::before_post(state, user.id.get(), &checked.texts[1]).await?;
    let title = crate::platform::plugins::filter_text(state, "item.before_create", "title", &checked.texts[0]).await?;
    let slug = slugify(&title);
    validate_title(state, &title).map_err(AppError::Validation)?;
    validate_body(state, &filtered).map_err(AppError::Validation)?;
    // A forum that requires tags cannot take a topic by email unless a watched word supplies them.
    let node_row = tree.node(node).ok_or(AppError::NotFound)?;
    let csv = checked.tags.join(",");
    check_tags(state, pool, node, node_row.require_tag, &csv).await?;
    super::rules::check_duplicate_title(state, pool, &title).await?;
    super::rules::check_duplicate_post(state, &user, &filtered).await?;
    let cooked = crate::platform::plugins::cook(state, &filtered).await;
    check_new_member_links(state, &user, cooked.links.len())?;
    check_image_limit(state, &user, &cooked.html).await?;
    check_content_rules(state, &user, &filtered, &cooked.html)?;
    let post = posts::NewPost {
        author_id: user.id.get(),
        raw: &filtered,
        cooked: &cooked.html,
        cooked_version: bf_cook::COOK_VERSION,
        ip: None,
        pending: checked.pending,
    };
    let id = posts::create_topic(pool, node, &title, &slug, &post).await.db()?;
    // Watched-word tags, as the site's form applies them.
    apply_tags(state, &actor, id, &csv, tree.perms(&actor, node).has(Perms::MODERATE)).await?;
    links::record_links(pool, id, 1, &cooked.links).await.db()?;
    state.forum.invalidate();
    if checked.pending {
        crate::community::notifications::tell_reviewers(state, node, user.id.get()).await;
    } else {
        after_post(state, &user, id, node, &title, &slug, 1, &filtered, &cooked.mentions).await;
    }
    Ok(())
}

/// A reply that arrived by email. The same rules as the site: the member must still be able to
/// read and reply, and limits, word screening and plugins all apply.
pub(crate) async fn reply_by_email(state: &AppState, user_id: i64, id: i64, body: &str) -> Result<(), AppError> {
    let pool = state.db()?;
    let user = users::user_by_id(pool, user_id.into()).await.db()?.ok_or(AppError::NotFound)?;
    // Everything a signed-in member would be stopped by on the site.
    if bf_db::accounts::users::is_suspended(pool, user.id.get()).await.db()? || !user.approved || user.legal_pending {
        return Err(AppError::Forbidden);
    }
    let actor = Actor {
        user_id: Some(user.id),
        groups: user.groups.clone(),
        is_admin: user.is_admin,
        silenced: user.silenced,
        moderated_nodes: user.moderated_nodes.clone(),
        denied: crate::platform::plugins::denials(state, user.id.get(), &user.groups).await,
    };
    let t = topics::topic_for(pool, id, Some(user.id.get())).await.db()?.ok_or(AppError::NotFound)?;
    let tree = state.forum.get(pool).await?;
    let perms = tree.perms(&actor, t.node_id);
    let archived_forum = tree.node(t.node_id).is_some_and(|n| n.kind == "archive");
    let moderator = perms.has(Perms::MODERATE);
    if !perms.has(Perms::READ)
        || !perms.has(Perms::REPLY)
        || t.state != "visible"
        || actor.silenced
        || ((t.locked || t.archived || archived_forum) && !moderator)
    {
        return Err(AppError::Forbidden);
    }
    validate_body(state, body).map_err(AppError::Validation)?;
    crate::forum::topic_tools::check_slow_mode(state, id, user.id.get(), moderator).await?;
    daily_limit(state, &user).await?;
    check_activity_limits(state, &user, Activity::Reply(id)).await?;
    let checked = screen(
        state,
        &tree,
        &user,
        &actor,
        t.node_id,
        &[body],
        None,
        posts::approved_post_count(state.db()?, user.id.get(), state.settings().int("approve_first_posts"))
            .await
            .db()?,
    )
    .map_err(AppError::Validation)?;
    let filtered = crate::platform::plugins::before_post(state, user.id.get(), &checked.texts[0]).await?;
    validate_body(state, &filtered).map_err(AppError::Validation)?;
    super::rules::check_duplicate_post(state, &user, &filtered).await?;
    let cooked = crate::platform::plugins::cook(state, &filtered).await;
    check_new_member_links(state, &user, cooked.links.len())?;
    check_image_limit(state, &user, &cooked.html).await?;
    check_content_rules(state, &user, &filtered, &cooked.html)?;
    let post = posts::NewPost {
        author_id: user.id.get(),
        raw: &filtered,
        cooked: &cooked.html,
        cooked_version: bf_cook::COOK_VERSION,
        ip: None,
        pending: checked.pending,
    };
    let number = posts::create_reply(pool, id, t.node_id, &post).await.db()?;
    links::record_links(pool, id, number, &cooked.links).await.db()?;
    state.forum.invalidate();
    if checked.pending {
        crate::community::notifications::tell_reviewers(state, t.node_id, user.id.get()).await;
    } else {
        after_post(state, &user, id, t.node_id, &t.title, &t.slug, number, &filtered, &cooked.mentions).await;
    }
    Ok(())
}
