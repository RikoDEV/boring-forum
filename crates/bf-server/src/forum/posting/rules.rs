//! What a new text must pass before it is saved: length and title checks, watched words and approval rules, image and daily post limits.

use crate::forum::tree::Tree;
use crate::http::i18n::{tr, trf};
use crate::http::render::DbResult;
use crate::state::AppState;
use bf_core::AppError;
use bf_core::perms::{Actor, Perms};
use bf_db::accounts::users;

pub(super) fn validate_body(state: &AppState, body: &str) -> Result<(), String> {
    let s = state.settings();
    let n = body.trim().chars().count() as i64;
    if n < s.int("min_post_length") {
        return Err(trf("Posts must be at least {n} characters.", &[("n", &s.int("min_post_length").to_string())]));
    }
    if n > s.int("max_post_length") {
        return Err(trf("Posts can be at most {n} characters.", &[("n", &s.int("max_post_length").to_string())]));
    }
    Ok(())
}

pub(crate) fn validate_title(state: &AppState, title: &str) -> Result<(), String> {
    let s = state.settings();
    if title.chars().any(bf_core::html::is_hidden_or_bidi) {
        return Err(trf("Titles cannot contain hidden or direction-changing characters.", &[]));
    }
    let n = title.trim().chars().count() as i64;
    if n < s.int("min_title_length") || n > s.int("max_title_length") {
        return Err(trf(
            "Titles must be {min}–{max} characters.",
            &[("min", &s.int("min_title_length").to_string()), ("max", &s.int("max_title_length").to_string())],
        ));
    }
    let max_emoji = s.int("max_emojis_in_title");
    if max_emoji > 0 && title.chars().filter(|c| is_emoji(*c)).count() as i64 > max_emoji {
        return Err(trf("Titles can have at most {n} emoji.", &[("n", &max_emoji.to_string())]));
    }
    let max_word = s.int("title_max_word_length");
    if max_word > 0 && title.split_whitespace().any(|w| w.chars().count() as i64 > max_word) {
        return Err(trf("Words in a title can be at most {n} characters long.", &[("n", &max_word.to_string())]));
    }
    if !s.bool("allow_all_caps_titles") {
        let letters = title.chars().filter(|c| c.is_alphabetic()).count();
        if letters >= 6 && !title.chars().any(char::is_lowercase) {
            return Err(tr("Titles cannot be written entirely in capital letters."));
        }
    }
    Ok(())
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF)
}

/// Content limits that depend on the text and on who wrote it: a member's first post, quotes,
/// and embedded media in a new member's post.
pub(super) fn check_content_rules(state: &AppState, user: &users::User, raw: &str, html: &str) -> Result<(), AppError> {
    let s = state.settings();
    if user.is_admin {
        return Ok(());
    }
    let first = s.int("min_first_post_length");
    if user.post_count == 0 && first > 0 && (raw.trim().chars().count() as i64) < first {
        return Err(AppError::invalid("Your first post must be at least {n} characters.", &[("n", &first)]));
    }
    let quotes = s.int("max_quotes_per_post");
    if quotes > 0 && html.matches("<blockquote").count() as i64 > quotes {
        return Err(AppError::invalid("A post can have at most {n} quotes.", &[("n", &quotes)]));
    }
    let media = s.int("new_user_max_media");
    if media > 0 && i64::from(user.post_count) < s.int("new_user_posts") {
        let n: usize = ["<img", "<video", "<audio", "<iframe"].iter().map(|t| html.matches(t).count()).sum();
        if n as i64 > media {
            return Err(AppError::invalid(
                "New members can include at most {n} images or embeds in a post.",
                &[("n", &media)],
            ));
        }
    }
    Ok(())
}

/// What a member is about to do, for the per-day and first-day limits.
pub(super) enum Activity {
    Topic,
    Reply(i64),
    Edit,
}

const TOPICS_TODAY: &str = "SELECT count(*) FROM items WHERE author_id = $1 AND created_at > now() - interval '1 day'";
// A reply posted anonymously is under the shared account, with its writer in `real_author_id`.
const REPLIES_TODAY: &str = "SELECT count(*) FROM posts WHERE (author_id = $1 OR real_author_id = $1) AND post_number > 1 AND action IS NULL
     AND created_at > now() - interval '1 day'";
const EDITS_TODAY: &str =
    "SELECT count(*) FROM post_revisions WHERE editor_id = $1 AND created_at > now() - interval '1 day'";

/// Per-day and first-day limits on topics, replies and edits, and the cap on replies in a row.
/// Administrators are exempt.
pub(super) async fn check_activity_limits(
    state: &AppState,
    user: &users::User,
    what: Activity,
) -> Result<(), AppError> {
    if user.is_admin {
        return Ok(());
    }
    let s = state.settings();
    let pool = state.db()?;
    let uid = user.id.get();
    let count = |sql: &'static str| async move {
        sqlx::query_scalar::<_, i64>(sql).bind(uid).fetch_one(pool).await.db()
    };
    let new_account = async {
        sqlx::query_scalar::<_, bool>("SELECT joined_at > now() - interval '1 day' FROM users WHERE id = $1")
            .bind(uid)
            .fetch_one(pool)
            .await
            .db()
    };
    match what {
        Activity::Topic => {
            let (day, first) = (s.int("max_topics_per_day"), s.int("max_topics_in_first_day"));
            if day > 0 || first > 0 {
                let made = count(TOPICS_TODAY).await?;
                if day > 0 && made >= day {
                    return Err(AppError::invalid(
                        "You can start {n} topics a day. Please come back tomorrow.",
                        &[("n", &day)],
                    ));
                }
                if first > 0 && made >= first && new_account.await? {
                    return Err(AppError::invalid(
                        "New members can start {n} topics in their first day.",
                        &[("n", &first)],
                    ));
                }
            }
        }
        Activity::Reply(item) => {
            let first = s.int("max_replies_in_first_day");
            if first > 0 && count(REPLIES_TODAY).await? >= first && new_account.await? {
                return Err(AppError::invalid(
                    "New members can post {n} replies in their first day.",
                    &[("n", &first)],
                ));
            }
            let row = s.int("max_consecutive_replies");
            if row > 0 {
                let authors: Vec<Option<i64>> = sqlx::query_scalar(
                    "SELECT coalesce(real_author_id, author_id) FROM posts WHERE item_id = $1 AND action IS NULL AND state <> 'deleted'
                     ORDER BY post_number DESC LIMIT $2",
                )
                .bind(item)
                .bind(row)
                .fetch_all(pool)
                .await
                .db()?;
                if authors.len() as i64 == row && authors.iter().all(|a| *a == Some(uid)) {
                    return Err(AppError::invalid(
                        "You have already replied {n} times in a row here. Wait for someone else to answer.",
                        &[("n", &row)],
                    ));
                }
            }
        }
        Activity::Edit => {
            let day = s.int("max_edits_per_day");
            if day > 0 && count(EDITS_TODAY).await? >= day {
                return Err(AppError::invalid(
                    "You can edit posts {n} times a day. Please come back tomorrow.",
                    &[("n", &day)],
                ));
            }
        }
    }
    Ok(())
}

/// Refuses a title that another visible topic already has, when the site forbids repeats.
/// The same words as the member's own post a few minutes ago: a double click or a paste spree.
pub(super) async fn check_duplicate_post(state: &AppState, user: &users::User, raw: &str) -> Result<(), AppError> {
    let minutes = state.settings().int("duplicate_post_minutes");
    if minutes <= 0 || user.is_admin || raw.trim().chars().count() < 10 {
        return Ok(());
    }
    let same: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM posts WHERE (author_id = $1 OR real_author_id = $1) AND state <> 'deleted'
           AND created_at > now() - make_interval(mins => $2::int) AND lower(btrim(raw)) = lower(btrim($3)))",
    )
    .bind(user.id.get())
    .bind(minutes as i32)
    .bind(raw)
    .fetch_one(state.db()?)
    .await
    .db()?;
    if same {
        return Err(AppError::Validation("You already posted exactly this a moment ago.".into()));
    }
    Ok(())
}

pub(super) async fn check_duplicate_title(state: &AppState, pool: &sqlx::PgPool, title: &str) -> Result<(), AppError> {
    if state.settings().bool("allow_duplicate_topic_titles") {
        return Ok(());
    }
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM items WHERE state = 'visible' AND lower(title) = lower($1))")
            .bind(title.trim())
            .fetch_one(pool)
            .await
            .db()?;
    if taken {
        return Err(AppError::Validation("A topic with this title already exists.".into()));
    }
    Ok(())
}

/// New members may put only a few links in a post, which keeps spam out of the first posts.
pub(super) fn check_new_member_links(state: &AppState, user: &users::User, links: usize) -> Result<(), AppError> {
    let s = state.settings();
    let max = s.int("max_links_per_post");
    if !user.is_admin && max > 0 && links as i64 > max {
        return Err(AppError::invalid("A post can have at most {n} links.", &[("n", &max)]));
    }
    if !user.is_admin
        && i64::from(user.post_count) < s.int("new_user_posts")
        && links as i64 > s.int("new_user_max_links")
    {
        return Err(AppError::invalid(
            "New members can include at most {n} links in a post.",
            &[("n", &s.int("new_user_max_links"))],
        ));
    }
    Ok(())
}

/// Images per post, by group or site setting.
pub(super) async fn check_image_limit(state: &AppState, user: &users::User, html: &str) -> Result<(), AppError> {
    let limits = bf_db::accounts::limits::limits(state.db()?, &user.groups).await.db()?;
    let max = limits.max_images_per_post.unwrap_or_else(|| state.settings().int("max_images_per_post"));
    if !user.is_admin && html.matches("<img").count() as i64 > max {
        return Err(AppError::invalid("A post can have at most {max} images.", &[("max", &max)]));
    }
    Ok(())
}

/// Result of screening new text against watched words and approval rules.
pub(super) struct Screened {
    pub(super) texts: Vec<String>,
    pub(super) pending: bool,
    /// From watched words with the `tag` action.
    pub(super) tags: Vec<String>,
}

/// Applies watched words to each text (title, body) and the "first N posts" rule.
/// Moderators of the forum skip both, so they can quote and discuss blocked words.
///
/// `edit_of` is the text being replaced. An edit is held again only for what it adds: a word
/// that needs approval, or a first link in a forum that holds posts with links. The "first N
/// posts" and "hold everything" rules were met when the post was approved. `approved_posts` is
/// the author's public post count, for the "first N posts" rule.
#[allow(clippy::too_many_arguments)]
pub(super) fn screen(
    state: &AppState,
    tree: &Tree,
    user: &users::User,
    actor: &Actor,
    node_id: i64,
    texts: &[&str],
    edit_of: Option<&str>,
    approved_posts: i64,
) -> Result<Screened, String> {
    crate::moderation::gate::account_ready(state, user)?;
    if tree.perms(actor, node_id).has(Perms::MODERATE) {
        return Ok(Screened { texts: texts.iter().map(|t| (*t).to_owned()).collect(), pending: false, tags: vec![] });
    }
    let node = tree.node(node_id);
    if node.is_some_and(|n| i32::from(n.min_trust_level) > i32::from(user.trust_level)) {
        let t = crate::http::i18n::lang();
        let need = i16::from(node.map_or(0, |n| n.min_trust_level));
        return Err(t.msgf(
            "Posting in this forum needs trust level {need} ({name}). You are at level {have}. Follow your progress at /trust.",
            &[
                ("need", &need.to_string()),
                ("name", &crate::community::trust::level_label(state, &t, need)),
                ("have", &user.trust_level.to_string()),
            ],
        ));
    }
    let limits_rule = node.map_or("none", |n| n.approve_mode.as_str());
    let linked = |t: &str| t.contains("http://") || t.contains("https://");
    let has_link = texts.iter().any(|t| linked(t));
    let mut pending = match edit_of {
        Some(old) => limits_rule == "links" && has_link && !linked(old),
        None => {
            approved_posts < state.settings().int("approve_first_posts")
                || limits_rule == "all"
                || (limits_rule == "links" && has_link)
        }
    };
    // Sites the administrator listed: a link to one is refused, or holds the post for review.
    let (bad_domain, review_domain) = crate::moderation::spam::link_verdict(state, &texts.join("\n"));
    if let Some(domain) = bad_domain {
        return Err(trf("Links to {domain} are not allowed here.", &[("domain", &domain)]));
    }
    pending |= review_domain
        && edit_of.is_none_or(|old| !crate::moderation::spam::link_verdict(state, old).1);
    let mut watched_tags = Vec::new();
    let mut out = Vec::with_capacity(texts.len());
    for t in texts {
        match bf_core::watched::apply(&tree.words, t) {
            bf_core::watched::Outcome::Blocked(word) => {
                return Err(trf("Your post contains a blocked word: {word}", &[("word", &word)]));
            }
            bf_core::watched::Outcome::Allow { text, needs_approval, tags, silence, .. } => {
                pending |= needs_approval;
                if silence {
                    crate::moderation::spam::silence_in_background(state, user.id.get(), "A watched word that silences its author was used.");
                }
                watched_tags.extend(tags);
                out.push(text);
            }
        }
    }
    Ok(Screened { texts: out, pending, tags: watched_tags })
}

/// What a remote (ActivityPub) shadow account's text got from the site's posting rules.
pub(crate) struct Federated {
    pub(crate) user: users::User,
    pub(crate) text: String,
    pub(crate) pending: bool,
}

/// The posting rules for text from a remote account, before it is cooked: the forum's REPLY or
/// CREATE permission for the shadow user's groups, no archive forums, body length, slow mode,
/// group and activity limits, trust level, first-post approval (counted from the shadow
/// account's own visible posts), listed domains, watched words and duplicate text. Any failure
/// is an `Err` the caller drops quietly: the sender learns nothing.
pub(crate) async fn screen_federated(
    state: &AppState,
    shadow: i64,
    node_id: i64,
    topic: Option<i64>,
    raw: &str,
) -> Result<Federated, String> {
    let pool = state.db().map_err(|_| "no database".to_owned())?;
    let mut user = users::user_by_id(pool, shadow.into())
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no shadow account")?;
    // A remote account cannot accept terms, set up 2FA or be an administrator.
    user.legal_pending = false;
    user.needs_2fa = false;
    user.is_admin = false;
    let actor = Actor {
        user_id: Some(user.id),
        groups: user.groups.clone(),
        is_admin: false,
        silenced: user.silenced || !user.approved,
        moderated_nodes: Vec::new(),
        ..Default::default()
    };
    let tree = state.forum.get(pool).await.map_err(|e| e.to_string())?;
    let node = tree.node(node_id).ok_or("no such forum")?;
    if node.kind == "archive" {
        return Err("archive forum".into());
    }
    let perms = tree.perms(&actor, node_id);
    let needed = if topic.is_some() { Perms::REPLY } else { Perms::CREATE };
    if !perms.has(needed) || actor.silenced {
        return Err("remote account may not post here".into());
    }
    let max = state.settings().int("max_post_length");
    if raw.trim().chars().count() as i64 > max {
        return Err("too long".into());
    }
    if let Some(t) = topic {
        crate::forum::topic_tools::check_slow_mode(state, t, user.id.get(), false).await.map_err(|_| "slow mode")?;
    }
    daily_limit(state, &user).await.map_err(|_| "daily limit")?;
    let activity = topic.map_or(Activity::Topic, Activity::Reply);
    check_activity_limits(state, &user, activity).await.map_err(|_| "activity limit")?;
    let approved = bf_db::forum::posts::approved_post_count(pool, user.id.get(), state.settings().int("approve_first_posts"))
        .await
        .map_err(|e| e.to_string())?;
    let screened = screen(state, &tree, &user, &actor, node_id, &[raw], None, approved)?;
    let text = screened.texts.into_iter().next().unwrap_or_default();
    check_duplicate_post(state, &user, &text).await.map_err(|_| "duplicate")?;
    Ok(Federated { user, text, pending: screened.pending })
}

/// The rules that need the cooked text: links and images of a new member, quotes, embeds.
pub(crate) async fn check_federated_cooked(
    state: &AppState,
    user: &users::User,
    raw: &str,
    cooked: &bf_cook::Cooked,
) -> Result<(), String> {
    check_new_member_links(state, user, cooked.links.len()).map_err(|_| "too many links")?;
    check_image_limit(state, user, &cooked.html).await.map_err(|_| "too many images")?;
    check_content_rules(state, user, raw, &cooked.html).map_err(|_| "content rules")?;
    Ok(())
}

/// Group limit on posts per day ("group limits").
pub(super) async fn daily_limit(state: &AppState, user: &users::User) -> Result<(), AppError> {
    let pool = state.db()?;
    let flood = state.settings().int("flood_wait_seconds");
    if flood > 0
        && !user.is_admin
        && let Some(since) = bf_db::forum::posts::seconds_since_last_post(pool, user.id.get()).await.db()?
        && since < flood
    {
        return Err(AppError::invalid(
            "Please wait {n} more seconds before posting again.",
            &[("n", &(flood - since))],
        ));
    }
    let limits = bf_db::accounts::limits::limits(pool, &user.groups).await.db()?;
    if let Some(max) = limits.posts_per_day
        && bf_db::accounts::limits::posts_today(pool, user.id).await.db()? >= max
    {
        return Err(AppError::invalid(
            "Your group can post {max} times a day. Please come back tomorrow.",
            &[("max", &max)],
        ));
    }
    Ok(())
}
