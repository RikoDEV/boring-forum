//! Email digests ("immediate or digest"): a summary of new topics since the
//! member's last digest, limited to forums that member can read.

use crate::state::AppState;
use bf_core::i18n::T;
use bf_core::perms::Actor;
use std::time::Duration;

/// Runs one pass; returns how many digests were queued.
pub async fn run_once(state: &AppState) -> Result<usize, bf_core::AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    // Claimed: each member is handed to one node, and their clock is already moved.
    let due = bf_db::platform::digests::due_digests(pool, 500).await.map_err(crate::http::render::db_err)?;
    let s = state.settings();
    let site = s.str("site_name");
    let mut queued = 0;
    for u in due {
        match queue_one(state, &tree, site, &u).await {
            Ok(sent) => queued += usize::from(sent),
            Err(e) => {
                // Give the claim back, so the next pass tries this member again.
                tracing::warn!(error = %e, user = u.id, "digest failed");
                if let Err(e) = bf_db::platform::digests::release_digest(pool, u.id, u.since).await {
                    tracing::warn!(error = %e, user = u.id, "digest claim not released");
                }
            }
        }
    }
    Ok(queued)
}

/// Builds and queues one member's digest, in their language. Nothing new still counts as done,
/// so the next digest covers a fresh period.
async fn queue_one(
    state: &AppState,
    tree: &crate::forum::tree::Tree,
    site: &str,
    u: &bf_db::platform::digests::DigestUser,
) -> Result<bool, bf_core::AppError> {
    let pool = state.db()?;
    let db = crate::http::render::db_err;
    let actor = Actor {
        user_id: Some(u.id.into()),
        groups: bf_core::ids::typed(u.groups.clone()),
        is_admin: u.is_admin,
        // What a plugin keeps this member from reading is kept out of the digest too.
        denied: crate::platform::plugins::denials_cached(u.id),
        ..Default::default()
    };
    let nodes = tree.readable(&actor);
    // A month is long: what was busy in it is worth more than what was merely new in it.
    let monthly = u.digest == "monthly";
    let topics = if monthly {
        bf_db::platform::digests::digest_popular(pool, u.id, &nodes, u.since, 20).await.map_err(db)?
    } else {
        bf_db::platform::digests::digest_topics(pool, u.id, &nodes, u.since, 20).await.map_err(db)?
    };
    let watched = bf_db::platform::digests::digest_watched(pool, u.id, &nodes, u.since, 20).await.map_err(db)?;
    let unread = bf_db::platform::digests::unread_notifications(pool, u.id).await.map_err(db)?;
    if topics.is_empty() && watched.is_empty() {
        // Unread notifications alone are not worth a mail: the member already got those.
        return Ok(false);
    }
    let locale: Option<String> =
        sqlx::query_scalar("SELECT locale FROM users WHERE id = $1").bind(u.id).fetch_one(pool).await.ok().flatten();
    let t = T::new(locale.as_deref().unwrap_or(bf_core::i18n::site_language(state.settings().str("default_locale"))));
    let base = state.config.base_url.as_str();
    let lines: String = topics
        .iter()
        .map(|(id, slug, title, replies)| {
            format!("- {title} ({})
  {base}{}
", t.n("count.replies", *replies), bf_core::slug::topic_path(*id, slug))
        })
        .collect();
    let watched_lines: String = watched
        .iter()
        .map(|(id, slug, title, n)| {
            format!("- {title} ({})
  {base}{}
", t.n("count.replies", *n), bf_core::slug::topic_path(*id, slug))
        })
        .collect();
    let mut sections = String::new();
    if !topics.is_empty() {
        let heading = if monthly { t.msg("Popular topics this month:") } else { t.msg("New topics since your last digest:") };
        sections.push_str(&format!("{heading}

{lines}
"));
    }
    if !watched.is_empty() {
        sections.push_str(&format!("{}

{watched_lines}
", t.msg("New replies in topics you watch:")));
    }
    if monthly {
        let joined = bf_db::platform::digests::new_members_since(pool, u.since).await.map_err(db)?;
        if joined > 0 {
            sections.push_str(&format!(
                "{}

",
                t.msgf("New members this month: {count}", &[("count", &joined.to_string())])
            ));
        }
    }
    if unread > 0 {
        sections.push_str(&format!(
            "{}
{base}/notifications

",
            t.msgf("You have {count} unread notifications.", &[("count", &unread.to_string())])
        ));
    }
    let token = bf_db::platform::digests::digest_token(pool, u.id).await.map_err(db)?;
    let unsubscribe = format!("{base}/unsubscribe/{token}");
    // The mail in the member's language: the catalog text, or the one an administrator wrote.
    let vars = [
        ("site", site),
        ("username", u.username.as_str()),
        ("topics", sections.as_str()),
        ("link", base),
        ("unsubscribe", unsubscribe.as_str()),
    ];
    let (subject, mut body) = crate::admin::mail_templates::render(&t, "digest", &vars);
    // A rewritten text that dropped the unsubscribe link still carries it.
    if !body.contains(&unsubscribe) {
        body.push_str("\n\n");
        body.push_str(&t.msgf("Unsubscribe from digests: {link}", &[("link", &unsubscribe)]));
        body.push('\n');
    }
    bf_db::platform::digests::mark_digest_sent(pool, &u.email, &subject, Some(&body), &unsubscribe)
        .await
        .map_err(db)?;
    Ok(true)
}

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            match run_once(&state).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(digests = n, "digests queued"),
                Err(e) => tracing::warn!(error = %e, "digest run failed"),
            }
        }
    });
}
