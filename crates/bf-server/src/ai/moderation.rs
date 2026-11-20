//! What the model makes of a new post: spam, abuse and mood. A post is never held up for the
//! answer. It is public first and judged right after; spam and abuse then go to the review queue
//! (or are held or hidden, as the administrator chose), and the mood goes into the admin report.

use super::client::{self, Chat, json_in};
use super::{DATA_RULE, actor, fenced, may_send};
use crate::state::AppState;
use bf_db::forum::posts;

/// The post that was just written.
pub struct NewPost {
    pub topic_id: i64,
    pub node_id: i64,
    pub number: i32,
    pub title: String,
    pub author_id: i64,
    pub author: String,
    pub trust_level: i16,
    pub is_admin: bool,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    pub spam: i16,
    pub toxicity: i16,
    pub sentiment: &'static str,
    pub emotion: String,
}

/// The model's JSON as labels; numbers are clamped and anything unexpected falls back to neutral.
pub(crate) fn parse_labels(text: &str) -> Option<Labels> {
    let v = json_in(text)?;
    let score = |k: &str| v[k].as_f64().map_or(0, |n| n.round().clamp(0.0, 100.0) as i16);
    let sentiment = match v["sentiment"].as_str().map(str::to_lowercase).as_deref() {
        Some("positive") => "positive",
        Some("negative") => "negative",
        _ => "neutral",
    };
    let emotion: String = v["emotion"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphabetic() || *c == '-')
        .take(20)
        .collect();
    Some(Labels { spam: score("spam"), toxicity: score("toxicity"), sentiment, emotion })
}

pub(crate) async fn classify(state: &AppState, title: &str, raw: &str) -> Option<Labels> {
    let system = format!(
        "You moderate a discussion forum. Judge the post and answer with ONLY a JSON object: \
         {{\"spam\": 0-100, \"toxicity\": 0-100, \"sentiment\": \"positive\"|\"neutral\"|\"negative\", \"emotion\": \"one English word\"}}. \
         spam: unsolicited advertising, link farming, scams, gibberish or off-topic promotion (an ordinary question or opinion is 0). \
         toxicity: insults, harassment, hate or threats (strong but civil disagreement is low). {DATA_RULE}"
    );
    // The title is a member's text too (the topic starter's): it goes inside the block.
    let prompt = format!(
        "<posts>\nTopic: {}\n\n{}\n</posts>",
        fenced(title),
        fenced(&client::clip(&bf_cook::strip_hidden(raw), 4000))
    );
    let done = client::chat(state, Chat::new("moderation", &system, prompt).max(80)).await.ok()?;
    parse_labels(&done.text)
}

/// Whether the post answers a yes/no question, for automation rules. `None` when the model could
/// not be asked.
pub(crate) async fn answers_yes(state: &AppState, question: &str, title: &str, raw: &str) -> Option<bool> {
    let system = format!(
        "You help moderate a discussion forum. Answer the question about the post with ONLY the word YES or NO. {DATA_RULE}"
    );
    let prompt = format!(
        "Question: {}\n<posts>\nTopic: {}\n\n{}\n</posts>",
        client::clip(question, 500),
        fenced(title),
        fenced(&client::clip(&bf_cook::strip_hidden(raw), 4000))
    );
    let done = client::chat(state, Chat::new("automation", &system, prompt).max(5)).await.ok()?;
    Some(done.text.trim().to_uppercase().starts_with("YES"))
}

/// A reply written by the model for an automation rule, following the administrator's instructions.
pub(crate) async fn write_reply(state: &AppState, instructions: &str, title: &str, raw: &str) -> Result<String, bf_core::AppError> {
    let system = format!(
        "You reply to a post on a discussion forum, on behalf of the site's staff. Follow these instructions: {}
         Be brief and polite, answer in the language of the post, use Markdown. {DATA_RULE}",
        client::clip(instructions, 2000)
    );
    let prompt = format!(
        "<posts>\nTopic: {}\n\n{}\n</posts>",
        fenced(title),
        fenced(&client::clip(&bf_cook::strip_hidden(raw), 4000))
    );
    Ok(client::chat(state, Chat::new("automation", &system, prompt).max(600)).await?.text)
}

pub(crate) async fn check_post(state: &AppState, p: &NewPost) {
    let s = state.settings();
    let (spam_on, tox_on, mood_on) =
        (s.bool("ai_spam_enabled"), s.bool("ai_toxicity_enabled"), s.bool("ai_sentiment_enabled"));
    let judged = (spam_on || tox_on) && i64::from(p.trust_level) <= s.int("ai_max_trust_level") && !p.is_admin;
    if (!judged && !mood_on) || p.author.starts_with("plugin-") || !may_send(state, p.node_id).await {
        return;
    }
    let Some(pool) = &state.pool else { return };
    let post_id: Option<i64> = sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = $2")
        .bind(p.topic_id)
        .bind(p.number)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some(post_id) = post_id else { return };
    let Some(labels) = classify(state, &p.title, &p.raw).await else { return };
    if let Err(e) = bf_db::ai::save_labels(pool, post_id, labels.spam, labels.toxicity, labels.sentiment, &labels.emotion).await {
        tracing::warn!(error = %e, "AI labels could not be saved");
    }
    let spam = judged && spam_on && i64::from(labels.spam) >= s.int("ai_spam_threshold");
    let toxic = judged && tox_on && i64::from(labels.toxicity) >= s.int("ai_toxicity_threshold");
    if !spam && !toxic {
        return;
    }
    let Some(bot) = actor(state).await else { return };
    let Ok(Some(post)) = posts::post_ref(pool, post_id).await else { return };
    let (reason, score) = if spam { ("spam", labels.spam) } else { ("inappropriate", labels.toxicity) };
    let note = format!("AI: {reason} {score}/100");
    let action = if spam { s.str("ai_spam_action").to_owned() } else { "flag".to_owned() };
    let done = match action.as_str() {
        "hold" if post.state == "visible" => posts::requeue_post(pool, post_id).await.map(|_| ()),
        "hide" => posts::set_hidden(pool, post_id, true, Some(&note), bot).await,
        _ => bf_db::moderation::review::report(pool, bot, &post, reason, Some(&note), 1.0).await.map(|_| ()),
    };
    match done {
        Ok(()) => {
            tracing::info!(post = post_id, reason, score, action = %action, "AI moderation acted");
            // The post was taken out of view by an automated decision; tell the author why.
            if matches!(action.as_str(), "hold" | "hide") && (action == "hide" || post.state == "visible") {
                crate::moderation::dsa::record(
                    state,
                    crate::moderation::dsa::Restriction {
                        user_id: p.author_id,
                        target_type: "post",
                        target_id: post_id,
                        action: "hidden",
                        ground: "terms",
                        category: if spam { "scams_and_fraud" } else { "scope_of_platform_service" },
                        facts: if spam {
                            "An automated check judged the post to be spam and hid it until a moderator reviews it."
                        } else {
                            "An automated check judged the post to be abusive and hid it until a moderator reviews it."
                        },
                        automated_detection: true,
                        automated_decision: true,
                        notice_id: None,
                        moderator_id: None,
                    },
                )
                .await;
            }
            if spam && matches!(action.as_str(), "hold" | "hide") {
                crate::moderation::spam::spam_hidden(state, p.author_id, "The AI judged a post of theirs to be spam.").await;
            }
            state.forum.invalidate();
            state.microcache.clear();
            crate::community::notifications::tell_reviewers(state, p.node_id, p.author_id).await;
        }
        Err(e) => tracing::warn!(error = %e, "AI moderation could not act"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_clamped_and_cleaned() {
        let l = parse_labels("```json\n{\"spam\": 140, \"toxicity\": -5, \"sentiment\": \"Negative\", \"emotion\": \"Anger!\"}\n```").unwrap();
        assert_eq!(l, Labels { spam: 100, toxicity: 0, sentiment: "negative", emotion: "anger".into() });
        let l = parse_labels("{\"spam\": 12.4}").unwrap();
        assert_eq!((l.spam, l.sentiment, l.emotion.as_str()), (12, "neutral", ""));
        assert!(parse_labels("I cannot help with that").is_none());
    }
}
