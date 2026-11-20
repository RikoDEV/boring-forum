//! Automation: trigger → conditions → actions, configured by admins.
//!
//! Triggers are a new topic, a new post, or a schedule (daily, weekly, monthly). Actions tag,
//! close or move the topic, reply to it, or start a topic. Replies and topics come from the
//! `plugin-automation` account, which cannot sign in and is never an admin. What it writes goes
//! through the plugin posting path, which reads no mentions and runs no automations, so rules
//! cannot set each other off.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_core::slug::slugify;
use bf_db::platform::automations::{self, Rule};
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/automations", get(list).post(create))
        .route("/api/v1/admin/automations/{id}", put(update).delete(remove))
}

const TRIGGERS: [&str; 5] = ["topic_created", "post_created", "daily", "weekly", "monthly"];
const ACTIONS: [&str; 8] = ["tag", "close", "move", "reply", "create_topic", "flag", "ai_reply", "ai_report"];

/// What a post-triggered rule looks at.
pub(crate) struct PostEvent<'a> {
    pub author: &'a str,
    pub trust_level: i16,
    pub topic_id: i64,
    pub node_id: i64,
    pub number: i32,
    pub title: &'a str,
    pub raw: &'a str,
}

fn matches(conditions: &Value, e: &PostEvent<'_>) -> bool {
    if let Some(forums) = conditions.get("forums").and_then(Value::as_array).filter(|f| !f.is_empty())
        && !forums.iter().filter_map(Value::as_i64).any(|f| f == e.node_id)
    {
        return false;
    }
    let tl = i64::from(e.trust_level);
    if conditions.get("min_trust_level").and_then(Value::as_i64).is_some_and(|m| tl < m) {
        return false;
    }
    if conditions.get("max_trust_level").and_then(Value::as_i64).is_some_and(|m| tl > m) {
        return false;
    }
    if let Some(words) = conditions.get("words").and_then(Value::as_array).filter(|w| !w.is_empty()) {
        let text = format!("{} {}", e.title, e.raw).to_lowercase();
        if !words
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .any(|w| text.contains(&w.to_lowercase()))
        {
            return false;
        }
    }
    true
}

/// A member's words as plain text inside Markdown: every punctuation mark is escaped, so a title
/// cannot become a link, a picture, a heading, a quote or BBCode in a post the automation account
/// signs. A zero-width space after `@` keeps it from being a mention or a mail address, and line
/// breaks become spaces.
fn plain_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '@' => out.push_str("\\@\u{200b}"),
            c if c.is_ascii_punctuation() => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// `{author}`, `{title}` and `{date}` in a rule's text. The author and the title are a member's
/// own words: where the result is cooked as a post (`markdown`), they go in as plain text.
fn fill(template: &str, author: &str, title: &str, markdown: bool) -> String {
    let (author, title) =
        if markdown { (plain_markdown(author), plain_markdown(title)) } else { (author.to_owned(), title.to_owned()) };
    template
        .replace("{author}", &author)
        .replace("{title}", &title)
        .replace("{date}", &chrono::Utc::now().format("%Y-%m-%d").to_string())
}

async fn bot(state: &AppState) -> Option<(i64, String)> {
    let pool = state.pool.as_ref()?;
    let id = bf_db::platform::plugins::ensure_bot(pool, "automation").await.ok()?;
    Some((id, bf_db::platform::plugins::bot_username("automation")))
}

/// Runs the rules for a new topic or post. Called after the post is public; errors are logged
/// and do not affect the post.
pub(crate) async fn on_post(state: &AppState, e: PostEvent<'_>) {
    let Some(pool) = &state.pool else { return };
    let trigger = if e.number == 1 { "topic_created" } else { "post_created" };
    let rules = match automations::for_trigger(pool, trigger).await {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => return,
        Err(err) => {
            tracing::warn!(error = %err, "automation rules failed to load");
            return;
        }
    };
    for rule in rules.iter().filter(|r| matches(&r.conditions, &e)) {
        if !ai_agrees(state, rule, &e).await {
            continue;
        }
        if let Err(err) = run_actions(state, rule, Some(&e)).await {
            tracing::warn!(rule = rule.id, error = ?err, "automation rule failed");
        }
        if let Err(err) = automations::ran(pool, rule.id).await {
            tracing::warn!(error = %err, "automation count failed");
        }
    }
}

/// A rule with an `ai_question` runs only when the model answers YES about the post. When the
/// model cannot be asked, the rule does not run: guessing could close or flag the wrong posts.
async fn ai_agrees(state: &AppState, rule: &Rule, e: &PostEvent<'_>) -> bool {
    let question = rule.conditions.get("ai_question").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if question.is_empty() {
        return true;
    }
    if !crate::ai::client::configured(state) || !crate::ai::may_send(state, e.node_id).await {
        return false;
    }
    crate::ai::moderation::answers_yes(state, question, e.title, e.raw).await == Some(true)
}

/// A reply from the automation account.
async fn bot_reply(state: &AppState, e: &PostEvent<'_>, text: &str) -> Result<(), AppError> {
    let pool = state.db()?;
    let (bot_id, bot_name) = bot(state).await.ok_or(AppError::NotFound)?;
    // Plugin text filters apply to the bot too; a refusal drops the reply.
    let Some(text) = crate::platform::plugins::before_post_system(state, bot_id, text).await else { return Ok(()) };
    let text = text.as_str();
    let cooked = crate::platform::plugins::cook(state, text).await;
    let post = bf_db::forum::posts::NewPost {
        author_id: bot_id,
        raw: text,
        cooked: &cooked.html,
        cooked_version: bf_cook::COOK_VERSION,
        ip: None,
        pending: false,
    };
    let number = bf_db::forum::posts::create_reply(pool, e.topic_id, e.node_id, &post).await.db()?;
    let slug: String = sqlx::query_scalar("SELECT slug FROM items WHERE id = $1")
        .bind(e.topic_id)
        .fetch_one(pool)
        .await
        .db()?;
    crate::forum::posting::after_system_post(state, bot_id, &bot_name, e.title, &slug, e.topic_id, e.node_id, number).await;
    Ok(())
}

/// From the maintenance tick: scheduled rules whose time has come.
pub(crate) async fn run_scheduled(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    match automations::claim_due(pool).await {
        Ok(rules) => {
            for rule in &rules {
                if let Err(err) = run_actions(state, rule, None).await {
                    tracing::warn!(rule = rule.id, error = ?err, "scheduled automation failed");
                }
            }
        }
        Err(err) => tracing::warn!(error = %err, "scheduled automations failed"),
    }
}

async fn run_actions(state: &AppState, rule: &Rule, e: Option<&PostEvent<'_>>) -> Result<(), AppError> {
    let pool = state.db()?;
    let actions = rule.actions.as_array().cloned().unwrap_or_default();
    for a in &actions {
        let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let n = |k: &str| a.get(k).and_then(Value::as_i64);
        match (a.get("type").and_then(Value::as_str).unwrap_or_default(), e) {
            ("tag", Some(e)) => {
                let mut tags: Vec<(String, String)> = bf_db::forum::tags::tags_of(pool, &[e.topic_id])
                    .await
                    .db()?
                    .into_iter()
                    .map(|(_, name, slug)| (name, slug))
                    .collect();
                for name in s("tags").split(',').map(str::trim).filter(|t| !t.is_empty()) {
                    let slug = slugify(name);
                    if !slug.is_empty() && !tags.iter().any(|t| t.1 == slug) {
                        tags.push((name.chars().take(40).collect(), slug));
                    }
                }
                bf_db::forum::tags::set_tags(pool, e.topic_id, &tags, true).await.db()?;
            }
            ("close", Some(e)) => {
                sqlx::query("UPDATE items SET locked = true WHERE id = $1")
                    .bind(e.topic_id)
                    .execute(pool)
                    .await
                    .db()?;
                state.hub.publish(&format!("item:{}", e.topic_id), "state", json!({ "action": "closed" }));
            }
            ("move", Some(e)) => {
                let to = n("forum").ok_or(AppError::NotFound)?;
                let tree = state.forum.get(pool).await?;
                if tree.node(to).is_some_and(|f| f.kind == "forum") && to != e.node_id {
                    let (bot_id, _) = bot(state).await.ok_or(AppError::NotFound)?;
                    bf_db::moderation::topics::move_topic(pool, e.topic_id, to, bot_id).await.db()?;
                }
            }
            ("reply", Some(e)) => {
                let text = fill(&s("text"), e.author, e.title, true);
                if text.trim().is_empty() {
                    continue;
                }
                bot_reply(state, e, &text).await?;
            }
            ("ai_reply", Some(e)) => {
                if !s("text").trim().is_empty() && crate::ai::may_send(state, e.node_id).await {
                    let text = crate::ai::moderation::write_reply(state, &s("text"), e.title, e.raw).await?;
                    bot_reply(state, e, &text).await?;
                }
            }
            ("flag", Some(e)) => {
                if let Some(bot) = crate::ai::actor(state).await {
                    let post: Option<i64> = sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = $2")
                        .bind(e.topic_id)
                        .bind(e.number)
                        .fetch_optional(pool)
                        .await
                        .db()?;
                    if let Some(post) = bf_db::forum::posts::post_ref(pool, post.unwrap_or(0)).await.db()? {
                        let reason = if s("reason") == "spam" { "spam" } else { "inappropriate" };
                        let note = format!("Automation: {}", s("text").chars().take(200).collect::<String>());
                        bf_db::moderation::review::report(pool, bot, &post, reason, Some(&note), 1.0).await.db()?;
                        crate::community::notifications::tell_reviewers(state, e.node_id, bot).await;
                    }
                }
            }
            ("create_topic" | "ai_report", _) => {
                let forum = n("forum").ok_or(AppError::NotFound)?;
                let tree = state.forum.get(pool).await?;
                if tree.node(forum).is_none_or(|f| f.kind != "forum") {
                    return Err(AppError::NotFound);
                }
                let (bot_id, bot_name) = bot(state).await.ok_or(AppError::NotFound)?;
                let (author, title) = e.map_or(("", ""), |e| (e.author, e.title));
                // A topic title is shown as text, never cooked, so it takes the words as they are.
                let new_title: String = fill(&s("title"), author, title, false).chars().take(200).collect();
                let text = if a.get("type").and_then(Value::as_str) == Some("ai_report") {
                    if !crate::ai::client::configured(state) || s("text").trim().is_empty() {
                        continue;
                    }
                    let days = n("days").unwrap_or(7).clamp(1, 90) as i32;
                    crate::ai::report::write(state, &s("text"), days).await?
                } else {
                    fill(&s("text"), author, title, true)
                };
                if new_title.trim().is_empty() || text.trim().is_empty() {
                    continue;
                }
                // Plugin text filters (body and title) apply to the bot too; a refusal drops the topic.
                let Some(text) = crate::platform::plugins::before_post_system(state, bot_id, &text).await else {
                    continue;
                };
                let Ok(new_title) =
                    crate::platform::plugins::filter_text(state, "item.before_create", "title", &new_title).await
                else {
                    continue;
                };
                let cooked = crate::platform::plugins::cook(state, &text).await;
                let post = bf_db::forum::posts::NewPost {
                    author_id: bot_id,
                    raw: &text,
                    cooked: &cooked.html,
                    cooked_version: bf_cook::COOK_VERSION,
                    ip: None,
                    pending: false,
                };
                let slug = slugify(&new_title);
                let item =
                    bf_db::forum::posts::create_topic(pool, forum, &new_title, &slug, &post).await.db()?;
                crate::forum::posting::after_system_post(state, bot_id, &bot_name, &new_title, &slug, item, forum, 1)
                    .await;
            }
            // Topic actions on a schedule have no topic: nothing to do.
            _ => {}
        }
    }
    state.forum.invalidate();
    state.microcache.clear();
    Ok(())
}

fn check(r: &Rule) -> Result<(), AppError> {
    let bad = |m: &str| Err(AppError::Validation(m.into()));
    if r.name.trim().is_empty() || r.name.chars().count() > 100 {
        return bad(en("A rule needs a name of up to 100 characters."));
    }
    if !TRIGGERS.contains(&r.trigger.as_str()) {
        return bad(en("Choose when the rule runs."));
    }
    let Some(actions) = r.actions.as_array().filter(|a| !a.is_empty() && a.len() <= 10) else {
        return bad(en("A rule needs 1 to 10 actions."));
    };
    let scheduled = matches!(r.trigger.as_str(), "daily" | "weekly" | "monthly");
    for a in actions {
        let kind = a.get("type").and_then(Value::as_str).unwrap_or_default();
        if !ACTIONS.contains(&kind) {
            return bad(en("Unknown action."));
        }
        if scheduled && !matches!(kind, "create_topic" | "ai_report") {
            return bad(en(
                "A scheduled rule can only start topics: there is no topic to tag, close, move or reply to.",
            ));
        }
        let text_len = a.get("text").and_then(Value::as_str).map_or(0, str::len);
        if text_len > 20_000 {
            return bad(en("An action's text is too long."));
        }
    }
    if !r.conditions.is_object() && !r.conditions.is_null() {
        return bad(en("Conditions must be an object."));
    }
    Ok(())
}

async fn list(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(automations::rules(state.db()?).await.db()?).into_response())
}

async fn create(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(r): Json<Rule>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check(&r)?;
    let id = automations::save(state.db()?, None, &r).await.db()?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(r): Json<Rule>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check(&r)?;
    if automations::save(state.db()?, Some(id), &r).await.db()?.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if !automations::delete(state.db()?, id).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event<'a>(raw: &'a str, tl: i16) -> PostEvent<'a> {
        PostEvent { author: "ann", trust_level: tl, topic_id: 1, node_id: 5, number: 1, title: "Help", raw }
    }

    #[test]
    fn conditions_narrow_rules() {
        assert!(matches(&json!({}), &event("x", 0)));
        assert!(matches(&json!({ "forums": [5], "words": ["Refund"] }), &event("I want a refund", 0)));
        assert!(!matches(&json!({ "forums": [6] }), &event("x", 0)));
        assert!(!matches(&json!({ "words": ["refund"] }), &event("hello", 0)));
        assert!(!matches(&json!({ "max_trust_level": 0 }), &event("x", 1)));
        assert_eq!(fill("Hi {author}, re {title}", "ann", "Help", true), "Hi ann, re Help");
    }
}
