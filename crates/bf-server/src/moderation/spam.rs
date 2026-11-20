//! Spam defences that act on what a member posted: reports that silence, the same site linked by
//! several new members, link domains the administrator listed, and watched words that flag.
//!
//! Automatic silences last `flag_auto_silence_hours`, reach only members up to
//! `flag_silence_max_trust_level` (never staff), are logged under the AI account, tell the member
//! and record the EU DSA statement of reasons, like a moderator's. A moderator lifts one from the
//! panel's Members tab.

use crate::state::AppState;
use bf_db::accounts::users::{self, User};
use bf_db::admin::members::{self, UserChange};
use bf_db::forum::posts;
use serde_json::json;

/// The sites linked from a text: lowercase host names without a leading `www.`, each once.
pub(crate) fn link_hosts(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, _) in raw.match_indices("http") {
        let rest = &raw[i..];
        let Some(after) = rest.strip_prefix("https://").or_else(|| rest.strip_prefix("http://")) else { continue };
        let end = after.find(|c: char| c.is_whitespace() || matches!(c, '/' | ')' | ']' | '>' | '"' | '\'' | '?' | '#' | '<')).unwrap_or(after.len());
        let authority = &after[..end];
        let host = authority.rsplit('@').next().unwrap_or(authority).split(':').next().unwrap_or_default();
        let host = host.trim_matches('.').to_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
        if host.contains('.') && !out.contains(&host) {
            out.push(host);
        }
    }
    out
}

/// Whether `host` is one of the domains in `list` (one per line or comma), or a subdomain of one.
pub(crate) fn domain_listed(host: &str, list: &str) -> bool {
    list.split(['\n', ',']).map(|d| d.trim().to_lowercase()).map(|d| d.trim_start_matches("*.").trim_start_matches("www.").to_owned()).filter(|d| !d.is_empty()).any(|d| {
        host == d || host.strip_suffix(d.as_str()).is_some_and(|front| front.ends_with('.'))
    })
}

/// What the link-domain lists say about a text: (blocked domain, held for review).
pub(crate) fn link_verdict(state: &AppState, raw: &str) -> (Option<String>, bool) {
    let s = state.settings();
    let (blocked, review) = (s.str("blocked_link_domains"), s.str("review_link_domains"));
    if blocked.trim().is_empty() && review.trim().is_empty() {
        return (None, false);
    }
    let hosts = link_hosts(raw);
    let bad = hosts.iter().find(|h| domain_listed(h, blocked)).cloned();
    let held = hosts.iter().any(|h| domain_listed(h, review));
    (bad, held)
}

/// Silences a member for the configured time. False when it was not done (off, staff, too trusted).
pub(crate) async fn silence_author(state: &AppState, user_id: i64, why: &str) -> bool {
    let s = state.settings();
    let hours = s.int("flag_auto_silence_hours");
    let Some(pool) = &state.pool else { return false };
    if hours <= 0 {
        return false;
    }
    let Ok(Some(user)) = users::user_by_id(pool, user_id.into()).await else { return false };
    if user.is_admin
        || user.groups.contains(&bf_core::ids::GroupId::MODERATORS)
        || i64::from(user.trust_level) > s.int("flag_silence_max_trust_level")
        || user.silenced
    {
        return false;
    }
    let Some(bot) = crate::ai::actor(state).await else { return false };
    silence_for(state, bot, user_id, hours, why, true).await
}

/// Silences a member for `hours`, logs it under `actor`, tells them and records the DSA statement.
pub(crate) async fn silence_for(state: &AppState, actor: i64, user_id: i64, hours: i64, why: &str, automatic: bool) -> bool {
    let Some(pool) = &state.pool else { return false };
    let until = chrono::Utc::now() + chrono::Duration::hours(hours);
    // An automatic silence never replaces a longer (or endless) one a moderator or an earlier flag set.
    if automatic {
        let longer: Option<bool> = sqlx::query_scalar(
            "SELECT silenced_until > now() + make_interval(hours => $2::int) FROM users WHERE id = $1",
        )
        .bind(user_id)
        .bind(i32::try_from(hours).unwrap_or(i32::MAX))
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten();
        if longer == Some(true) {
            return false;
        }
    }
    let change = UserChange { silenced_until: Some(Some(until)), ..Default::default() };
    match members::update_user(pool, user_id, &change, actor).await {
        Ok(true) => {}
        Ok(false) => return false,
        Err(e) => {
            tracing::warn!(error = %e, "silence failed");
            return false;
        }
    }
    let _ = bf_db::admin::audit::record(
        pool,
        bf_db::admin::audit::Entry::new("mod.silence")
            .by(actor)
            .on(format!("user:{user_id}"))
            .details(json!({ "reason": why, "hours": hours, "automatic": automatic })),
    )
    .await;
    crate::community::notifications::send(state, actor, &[user_id], "silenced", &json!({ "url": "/settings" })).await;
    crate::moderation::dsa::record(
        state,
        crate::moderation::dsa::Restriction {
            user_id,
            target_type: "account",
            target_id: user_id,
            action: "silenced",
            ground: "terms",
            category: "scope_of_platform_service",
            facts: why,
            automated_detection: automatic,
            automated_decision: automatic,
            notice_id: None,
            moderator_id: (!automatic).then_some(actor),
        },
    )
    .await;
    state.microcache.clear();
    true
}

/// For synchronous code (the posting checks): silence in the background.
pub(crate) fn silence_in_background(state: &AppState, user_id: i64, why: &'static str) {
    let state = state.clone();
    tokio::spawn(async move {
        silence_author(&state, user_id, why).await;
    });
}

/// A post of this member was hidden as spam: silence them if the settings say so.
pub(crate) async fn spam_hidden(state: &AppState, author: i64, why: &str) {
    silence_author(state, author, why).await;
}

/// Reports a post as spam from the AI account, so it reaches the review queue.
pub(crate) async fn flag_as_spam(state: &AppState, post_id: i64, note: &str) {
    let Some(pool) = &state.pool else { return };
    let Some(bot) = crate::ai::actor(state).await else { return };
    let Ok(Some(post)) = posts::post_ref(pool, post_id).await else { return };
    match bf_db::moderation::review::report(pool, bot, &post, "spam", Some(note), 1.5).await {
        Ok(_) => crate::community::notifications::tell_reviewers(state, post.node_id, post.author_id.unwrap_or(0)).await,
        Err(e) => tracing::warn!(error = %e, "spam flag failed"),
    }
}

/// After a post is public: watched words that flag it, and the same site linked by many new members.
pub(crate) async fn after_post(state: &AppState, author: &User, topic_id: i64, number: i32, raw: &str) {
    let Some(pool) = &state.pool else { return };
    if author.is_admin || author.groups.contains(&bf_core::ids::GroupId::MODERATORS) {
        return;
    }
    let post_id: Option<i64> = sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = $2")
        .bind(topic_id)
        .bind(number)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some(post_id) = post_id else { return };
    if let Ok(tree) = state.forum.get(pool).await
        && matches!(bf_core::watched::apply(&tree.words, raw), bf_core::watched::Outcome::Allow { flag: true, .. })
    {
        flag_as_spam(state, post_id, "A watched word marks this as spam.").await;
        return;
    }
    let s = state.settings();
    let threshold = s.int("new_user_spam_host_threshold");
    if threshold <= 0 || i64::from(author.trust_level) > s.int("flag_silence_max_trust_level") {
        return;
    }
    let own = url::Url::parse(&state.config.base_url).ok().and_then(|u| u.host_str().map(str::to_lowercase));
    for host in link_hosts(raw).into_iter().filter(|h| Some(h) != own.as_ref()).take(5) {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT p.author_id) FROM posts p JOIN users u ON u.id = p.author_id
             WHERE p.created_at > now() - interval '7 days' AND p.state <> 'deleted' AND u.trust_level <= $2
               AND position(lower($1) in lower(p.raw)) > 0",
        )
        .bind(&host)
        .bind(s.int("flag_silence_max_trust_level") as i16)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        if count >= threshold {
            let note = format!("{count} new members linked to {host} this week.");
            flag_as_spam(state, post_id, &note).await;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_come_out_of_links_in_any_markup() {
        let raw = "see https://www.Example.com/a?b=1 and [x](http://sub.spam.example:8080/p) <https://user:pw@x.test/> https://a.b.";
        assert_eq!(link_hosts(raw), vec!["example.com", "sub.spam.example", "x.test", "a.b"]);
        assert!(link_hosts("no links, httpd is a word, http://localhost/").is_empty());
    }

    #[test]
    fn listed_domains_include_their_subdomains_only() {
        let list = "spam.example\n*.casino.test, WWW.Bad.org";
        assert!(domain_listed("spam.example", list));
        assert!(domain_listed("shop.spam.example", list));
        assert!(domain_listed("casino.test", list));
        assert!(domain_listed("bad.org", list));
        assert!(!domain_listed("notspam.example", list));
        assert!(!domain_listed("spam.example.com", list));
        assert!(!domain_listed("anything.com", ""));
    }
}
