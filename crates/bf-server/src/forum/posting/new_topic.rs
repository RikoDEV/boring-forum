//! Starting a topic: where the viewer may post, tags, pins and the new-topic handler.

use crate::forum::tree::Tree;
use crate::http::i18n::tr;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::moderation::review::awaiting_approval;
use crate::moderation::topics::moderated_topic;
use crate::state::AppState;
use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_core::perms::{Actor, Perms};
use bf_core::slug::{forum_path, slugify, topic_path};
use bf_db::forum::topics::TopicFlag;
use bf_db::forum::{links, posts, tags, topics};
use serde::Deserialize;

use super::{
    Activity, after_post, check_activity_limits, check_content_rules, check_duplicate_title, check_image_limit,
    check_new_member_links, daily_limit, screen, validate_body, validate_title,
};

/// Forums the viewer can start a topic in, grouped by category with subforums indented.
pub(crate) fn create_targets(tree: &Tree, actor: &Actor) -> Vec<(String, Vec<(i64, String)>)> {
    fn walk(tree: &Tree, actor: &Actor, parent: i64, depth: usize, out: &mut Vec<(i64, String)>) {
        for n in tree.nodes.iter().filter(|n| n.parent_id == Some(parent)) {
            if n.kind == "forum" && tree.perms(actor, n.id).has(Perms::CREATE) {
                let mark =
                    if depth == 0 { String::new() } else { format!("{}↳ ", "\u{a0}\u{a0}\u{a0}".repeat(depth)) };
                out.push((n.id, format!("{mark}{}", n.title)));
            }
            walk(tree, actor, n.id, depth + 1, out);
        }
    }
    let mut groups = Vec::new();
    let mut loose = Vec::new();
    for n in tree.nodes.iter().filter(|n| n.parent_id.is_none()) {
        if n.kind == "category" {
            let mut list = Vec::new();
            walk(tree, actor, n.id, 0, &mut list);
            if !list.is_empty() {
                groups.push((n.title.clone(), list));
            }
        } else if n.kind == "forum" {
            if tree.perms(actor, n.id).has(Perms::CREATE) {
                loose.push((n.id, n.title.clone()));
            }
            walk(tree, actor, n.id, 1, &mut loose);
        }
    }
    if !loose.is_empty() {
        groups.push((tr("Other forums"), loose));
    }
    groups
}

/// The form lives on the forum page as a bottom sheet; this address only leads there.
pub(super) async fn new_topic_form(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    v.require_user()?;
    let tree = state.forum.get(state.db()?).await?;
    // The redirect carries the node's slug, which is made from its title, so the permission
    // check has to happen before it is built: without it this route reported the name of any
    // forum, including one the member cannot see. 404, not 403, so the answer does not
    // reveal that the forum exists.
    let node = tree
        .node(id)
        .filter(|n| n.kind == "forum" && tree.perms(&v.actor, n.id).has(Perms::READ))
        .ok_or(AppError::NotFound)?;
    Ok(Redirect::to(&format!("{}#new-topic", forum_path(node.id, &node.slug))).into_response())
}

#[derive(Deserialize)]
pub(super) struct TopicForm {
    pub(super) csrf: String,
    #[serde(default)]
    /// Text, because browsers send an empty number field as an empty string.
    pub(super) publish_minutes: String,
    pub(super) title: String,
    pub(super) body: String,
    #[serde(default)]
    pub(super) prefix: String,
    #[serde(default)]
    pub(super) tags: String,
    /// An event for the topic (the calendar): the date starts it; the rest is optional.
    #[serde(default)]
    pub(super) event_date: String,
    #[serde(default)]
    pub(super) event_start: String,
    #[serde(default)]
    pub(super) event_end: String,
    #[serde(default)]
    pub(super) event_timezone: String,
    #[serde(default)]
    pub(super) event_location: String,
    #[serde(default)]
    pub(super) event_repeat: String,
    #[serde(default)]
    pub(super) event_repeat_until: String,
}

/// Display name and URL slug of a tag, or `None` if nothing usable remains.
pub(crate) fn clean_tag(state: &AppState, raw: &str) -> Option<(String, String)> {
    let s = state.settings();
    let max = usize::try_from(s.int("max_tag_length")).unwrap_or(40).clamp(1, 40);
    let name: String = raw.trim().trim_start_matches('#').chars().filter(|c| !c.is_control()).take(max).collect();
    let name = name.trim();
    let name = if s.bool("force_lowercase_tags") { name.to_lowercase() } else { name.to_owned() };
    let slug = slugify(&name);
    (!name.is_empty() && !slug.is_empty()).then_some((name, slug))
}

/// The tags in a comma-separated list, each once (a repeat anywhere in the list is dropped).
pub(crate) fn tag_list(state: &AppState, csv: &str) -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    csv.split(',').filter_map(|t| clean_tag(state, t)).filter(|t| seen.insert(t.1.clone())).collect()
}

/// The site-wide limits on how many tags a topic carries.
fn check_tag_count(state: &AppState, n: usize) -> Result<(), AppError> {
    let s = state.settings();
    let n = n as i64;
    if n > s.int("max_tags_per_topic") {
        return Err(AppError::invalid("A topic can have at most {n} tags.", &[("n", &s.int("max_tags_per_topic"))]));
    }
    if n < s.int("min_tags_per_topic") {
        return Err(AppError::invalid("A topic needs at least {n} tags.", &[("n", &s.int("min_tags_per_topic"))]));
    }
    Ok(())
}

/// Applies a comma-separated tag list; new tags need `members_create_tags` or staff rights.
pub(crate) async fn apply_tags(
    state: &AppState,
    actor: &Actor,
    item: i64,
    csv: &str,
    moderator: bool,
) -> Result<(), AppError> {
    let s = state.settings();
    let tags = tag_list(state, csv);
    check_tag_count(state, tags.len())?;
    let slugs: Vec<String> = tags.iter().map(|t| t.1.clone()).collect();
    crate::forum::tags::check_tag_rules(state, item, &slugs).await?;
    let create = moderator || actor.is_admin || s.bool("members_create_tags");
    tags::set_tags(state.db()?, item, &tags, create).await.db()
}

#[derive(Deserialize)]
pub(super) struct PinForm {
    pub(super) csrf: String,
    #[serde(default)]
    pub(super) days: String,
    #[serde(default)]
    pub(super) global: String,
}

/// Pins a topic until a date and/or across the site. Global pins need an admin.
pub(super) async fn pin_options(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<PinForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, _) = moderated_topic(&state, &v, id).await?;
    let global = f.global == "1";
    if global && !v.actor.is_admin {
        return Err(AppError::Forbidden);
    }
    let pool = state.db()?;
    let days = f.days.trim().parse::<i32>().ok().filter(|d| (1..=3650).contains(d));
    topics::set_topic_flag(pool, id, TopicFlag::Pinned, true, user.id.get()).await.db()?;
    topics::set_pin_options(pool, id, days, global).await.db()?;
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(id, &t.slug)).into_response())
}

pub(super) async fn new_topic(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<TopicForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let node = tree.node(id).filter(|n| n.kind == "forum").ok_or(AppError::NotFound)?;
    let perms = tree.perms(&v.actor, id);
    if !perms.has(Perms::CREATE) {
        // A forum the member cannot even read must answer as if it did not exist: a 403 here
        // and a 404 for an unknown id would let an outsider enumerate private forums.
        return Err(if perms.has(Perms::READ) { AppError::Forbidden } else { AppError::NotFound });
    }
    if let Err(e) = validate_title(&state, &f.title).and_then(|_| validate_body(&state, &f.body)) {
        return Err(AppError::Validation(e));
    }
    // An event is checked before the topic exists: a bad date must not leave a topic behind.
    let event = if f.event_date.trim().is_empty() {
        None
    } else {
        if v.actor.silenced {
            return Err(AppError::Forbidden);
        }
        Some(
            crate::community::events::check_event(
                &state,
                &v,
                &crate::community::events::EventInput {
                    date: &f.event_date,
                    start_time: &f.event_start,
                    end_time: &f.event_end,
                    timezone: &f.event_timezone,
                    location: &f.event_location,
                    recurrence: &f.event_repeat,
                    repeat_until: &f.event_repeat_until,
                    capacity: "",
                },
            )
            .await?,
        )
    };
    daily_limit(&state, user).await?;
    check_activity_limits(&state, user, Activity::Topic).await?;
    let checked = match screen(
        &state,
        &tree,
        user,
        &v.actor,
        id,
        &[f.title.trim(), &f.body],
        None,
        posts::approved_post_count(
            state.db()?,
            user.id.get(),
            state.settings().int("approve_first_posts"),
        )
        .await
        .db()?,
    ) {
        Ok(c) => c,
        Err(e) => {
            return Err(AppError::Validation(e));
        }
    };
    super::rules::check_duplicate_post(&state, user, &checked.texts[1]).await?;
    let filtered = crate::platform::plugins::before_post(&state, user.id.get(), &checked.texts[1]).await?;
    let filtered_title =
        crate::platform::plugins::filter_text(&state, "item.before_create", "title", &checked.texts[0]).await?;
    let (title, body) = (filtered_title.as_str(), filtered.as_str());
    // The final tag set (typed plus watched-word tags) is checked before anything is saved, or a
    // refusal after the commit would leave a topic behind and a retry would make a second one.
    let mut csv = f.tags.clone();
    for t in &checked.tags {
        csv.push(',');
        csv.push_str(t);
    }
    check_tags(&state, pool, id, node.require_tag, &csv).await?;
    validate_title(&state, title).map_err(AppError::Validation)?;
    check_duplicate_title(&state, pool, title).await?;
    validate_body(&state, body).map_err(AppError::Validation)?;
    let slug = slugify(title);
    let cooked = crate::platform::plugins::cook(&state, body).await;
    check_new_member_links(&state, user, cooked.links.len())?;
    check_image_limit(&state, user, &cooked.html).await?;
    check_content_rules(&state, user, body, &cooked.html)?;
    let post = posts::NewPost {
        author_id: user.id.get(),
        raw: body,
        cooked: &cooked.html,
        cooked_version: bf_cook::COOK_VERSION,
        ip: v.ip,
        pending: checked.pending,
    };
    // Moderators can hold a new topic and publish it later.
    let publish_in = f
        .publish_minutes
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|m| *m > 0 && tree.perms(&v.actor, id).has(Perms::MODERATE));
    let post = posts::NewPost { pending: post.pending || publish_in.is_some(), ..post };
    let topic_id = posts::create_topic(pool, id, title, &slug, &post).await.db()?;
    if let Some(minutes) = publish_in {
        // Waiting for its time is not waiting for a reviewer: no review item, no early approval.
        bf_db::forum::tools::drop_scheduled_review(pool, topic_id).await.db()?;
        bf_db::forum::tools::set_timer(pool, topic_id, "publish", minutes.min(5_256_000), user.id.get())
            .await
            .db()?;
    }
    if let Some(e) = &event {
        bf_db::community::events::set_event(pool, topic_id, &e.as_new()).await.db()?;
    }
    let allowed = bf_db::admin::forums::forum_prefixes(pool, id).await.db()?;
    if let Some(px) = allowed.iter().find(|p| **p == f.prefix) {
        bf_db::forum::topics::set_prefix(pool, topic_id, Some(px)).await.db()?;
    }
    apply_tags(&state, &v.actor, topic_id, &csv, tree.perms(&v.actor, id).has(Perms::MODERATE)).await?;
    bf_db::forum::uploads::delete_draft(pool, user.id.get(), &format!("new-topic:{id}")).await.db()?;
    links::record_links(pool, topic_id, 1, &cooked.links).await.db()?;
    state.forum.invalidate();
    if publish_in.is_some() {
        return Ok(Redirect::to("/scheduled").into_response());
    }
    if checked.pending {
        crate::community::notifications::tell_reviewers(&state, id, user.id.get()).await;
        return Ok(awaiting_approval(&state, &v));
    }
    after_post(&state, user, topic_id, id, title, &slug, 1, body, &cooked.mentions).await;
    Ok(Redirect::to(&topic_path(topic_id, &slug)).into_response())
}

/// The tag rules, checked before anything is saved: a required tag, the per-topic maximum and the
/// forum's own tag rules.
pub(super) async fn check_tags(
    state: &AppState,
    pool: &sqlx::PgPool,
    forum: i64,
    require_tag: bool,
    tags_csv: &str,
) -> Result<(), AppError> {
    if require_tag && tag_list(state, tags_csv).is_empty() {
        return Err(AppError::Validation("Topics in this forum need at least one tag.".into()));
    }
    // Refused now, not after the topic is committed, or a retry would make a duplicate.
    check_tag_count(state, tag_list(state, tags_csv).len())?;
    let slugs: Vec<String> = tag_list(state, tags_csv).into_iter().map(|t| t.1).collect();
    if let Some(problem) = bf_db::forum::tags::tag_rule_problem_in_node(pool, forum, &slugs).await.db()? {
        return Err(AppError::Validation(problem));
    }
    Ok(())
}
