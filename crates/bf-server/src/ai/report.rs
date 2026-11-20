//! Scheduled reports: the model writes a short review of what happened on the forum lately, for
//! an automation rule to post as a topic.

use super::client::{self, Chat};
use super::{DATA_RULE, language_name};
use crate::state::AppState;
use bf_core::AppError;
use bf_core::slug::topic_path;

/// A report on the last `days` days, following the administrator's instructions. The numbers
/// cover public forums (every forum when `ai_public_only` is off); the topics it names always
/// come from forums guests can read, because the report is posted where others read it.
pub(crate) async fn write(state: &AppState, instructions: &str, days: i32) -> Result<String, AppError> {
    let pool = state.pool.as_ref().ok_or(AppError::NotFound)?;
    let tree = state.forum.get(pool).await?;
    let public = tree.readable(&bf_core::perms::Actor { groups: vec![1.into()], ..Default::default() });
    let nodes: Vec<i64> =
        if state.settings().bool("ai_public_only") { public.clone() } else { tree.nodes.iter().map(|n| n.id).collect() };
    let ((topics, posts, members), busiest) = bf_db::ai::activity(pool, days, &nodes).await.map_err(AppError::internal)?;
    let busiest: Vec<_> = busiest.into_iter().filter(|b| public.contains(&b.3)).collect();
    let moods = bf_db::ai::mood_report(pool, days, 85, 80).await.map_err(AppError::internal)?;
    let sum = |f: fn(&bf_db::ai::MoodRow) -> i64| moods.iter().map(f).sum::<i64>();
    let mut facts = format!(
        "Period: the last {days} days.\nNew topics: {topics}. New replies and posts: {posts}. New members: {members}.\n\
         Mood of judged posts: {} positive, {} neutral, {} negative; {} flagged as abusive.\nBusiest topics (title, new posts, link):\n",
        sum(|m| m.positive),
        sum(|m| m.neutral),
        sum(|m| m.negative),
        sum(|m| m.toxic)
    );
    for (id, title, fresh, _) in &busiest {
        let slug: String = sqlx::query_scalar("SELECT slug FROM items WHERE id = $1").bind(id).fetch_one(pool).await.map_err(AppError::internal)?;
        facts.push_str(&format!("- {} ({fresh}) {}{}\n", super::fenced(title), state.config.base_url, topic_path(*id, &slug)));
    }
    let lang = language_name(bf_core::i18n::site_language(state.settings().str("default_locale")));
    let system = format!(
        "You write a short, friendly community report for a forum's staff or members, in {lang}, in Markdown. \
         Use only the facts given; do not invent numbers or topics. Link topics with Markdown links. {DATA_RULE}\n\nInstructions from the administrator:\n{}",
        client::clip(instructions, 2000)
    );
    let prompt = format!("<posts>\n{facts}</posts>");
    Ok(client::chat(state, Chat::new("report", &system, prompt).max(1200)).await?.text)
}
