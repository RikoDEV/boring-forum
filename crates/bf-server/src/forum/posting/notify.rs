//! After a post is saved: following, mentions, watchers, mailing lists, live events and what plugin posts announce.

use crate::state::AppState;
use bf_core::slug::topic_path;
use bf_db::accounts::users;
use bf_db::community::mentions;
use serde_json::json;

/// After a visible post: auto-follow, notify mentioned users and watchers (in app
/// and/or email, per their preferences), push the live event and drop guest caches.
/// A held post has gone public: mentions, watchers and webhooks hear about it only now.
pub(crate) async fn announce_post(state: &AppState, item: i64, number: i32) {
    // (author, forum, title, slug, raw)
    type Row = (i64, i64, String, String, String);
    let Some(pool) = &state.pool else { return };
    let row: Result<Option<Row>, _> = sqlx::query_as(
        "SELECT p.author_id, i.node_id, i.title, i.slug, p.raw FROM posts p JOIN items i ON i.id = p.item_id
         WHERE p.item_id = $1 AND p.post_number = $2 AND p.author_id IS NOT NULL",
    )
    .bind(item)
    .bind(number)
    .fetch_optional(pool)
    .await;
    let Ok(Some((author, node, title, slug, raw))) = row else { return };
    let Ok(Some(user)) = users::user_by_id(pool, author.into()).await else { return };
    let cooked = crate::platform::plugins::cook(state, &raw).await;
    after_post(state, &user, item, node, &title, &slug, number, &raw, &cooked.mentions).await;
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn after_post(
    state: &AppState,
    author: &users::User,
    topic_id: i64,
    node_id: i64,
    topic_title: &str,
    slug: &str,
    number: i32,
    raw: &str,
    mentions: &[String],
) {
    let Some(pool) = &state.pool else { return };
    let url = format!("{}#post-{number}", topic_path(topic_id, slug));
    let level =
        if number == 1 { bf_db::community::follows::Level::Watch } else { bf_db::community::follows::Level::Track };
    if let Err(e) = bf_db::community::follows::follow_if_unset(pool, author.id.get(), topic_id, level).await {
        tracing::warn!(error = %e, "auto-follow failed");
    }
    let abs = format!("{}{url}", state.config.base_url);
    let mut mentioned: Vec<i64> = Vec::new();
    let mentions = &mentions[..mentions.len().min(state.settings().int("max_mentions_per_post") as usize)];
    if !mentions.is_empty() {
        match users::user_ids_by_names(pool, mentions).await {
            Ok(ids) => mentioned = bf_core::ids::raw(&ids).into_iter().filter(|i| *i != author.id).collect(),
            Err(e) => tracing::error!(error = %e, "mention lookup failed"),
        }
        // Names that are also groups notify their members, if the author is staff.
        match mentions::group_mention_recipients(pool, &bf_core::ids::raw(&author.groups), mentions, 500).await {
            Ok(ids) => {
                mentioned.extend(ids.into_iter().filter(|i| *i != author.id));
                mentioned.sort_unstable();
                mentioned.dedup();
            }
            Err(e) => tracing::error!(error = %e, "group mention lookup failed"),
        }
    }
    let watchers = if number > 1 {
        bf_db::community::follows::watchers(pool, topic_id, node_id, author.id.get()).await.unwrap_or_else(|e| {
            tracing::error!(error = %e, "watcher lookup failed");
            vec![]
        })
    } else {
        // A new topic notifies watchers of its forum.
        bf_db::community::follows::watchers(pool, 0, node_id, author.id.get()).await.unwrap_or_default()
    };
    let told = crate::community::notifications::post_created(
        state,
        crate::community::notifications::PostNews {
            author_id: author.id.get(),
            author: &author.username,
            topic_id,
            node_id,
            title: topic_title,
            number,
            url: &url,
            mentioned,
            watchers,
        },
    )
    .await;
    if state.settings().bool("mailing_list_mode") {
        mail_to_list(state, pool, author, topic_id, node_id, topic_title, number, &url, &abs, &told).await;
    }
    let event = if number == 1 { "topic.created" } else { "post.created" };
    let payload = json!({ "topic_id": topic_id, "post_number": number, "title": topic_title, "url": abs, "author": author.username, "node_id": node_id });
    crate::platform::webhooks::emit(state, event, payload).await;
    state.hub.publish(&format!("item:{topic_id}"), "post", json!({ "post_number": number, "by": author.username }));
    publish_activity(state, node_id, topic_id, number);
    state.microcache.clear();
    if number == 1 {
        crate::platform::indexnow::topic_published(state, node_id, topic_id, slug);
    }
    crate::platform::automation::on_post(
        state,
        crate::platform::automation::PostEvent {
            author: &author.username,
            trust_level: author.trust_level,
            topic_id,
            node_id,
            number,
            title: topic_title,
            raw,
        },
    )
    .await;
    crate::moderation::spam::after_post(state, author, topic_id, number, raw).await;
    crate::ai::on_post(
        state,
        crate::ai::moderation::NewPost {
            topic_id,
            node_id,
            number,
            title: topic_title.to_owned(),
            author_id: author.id.get(),
            author: author.username.clone(),
            trust_level: author.trust_level,
            is_admin: author.is_admin,
            raw: raw.to_owned(),
        },
    );
}

/// Mailing-list mode: the whole post to every member who chose it and can read the forum, except
/// those who already get an email about this post as a mention, quote or reply. It goes out like
/// notification mail: held for quiet hours and the email delay, in each member's language.
#[allow(clippy::too_many_arguments)]
pub(super) async fn mail_to_list(
    state: &AppState,
    pool: &sqlx::PgPool,
    author: &users::User,
    topic_id: i64,
    node_id: i64,
    title: &str,
    number: i32,
    url: &str,
    abs: &str,
    told: &[(&str, Vec<i64>)],
) {
    let mut ids =
        match bf_db::community::notifications::mailing_list_members(pool, author.id.get(), topic_id, node_id).await {
            Ok(ids) if !ids.is_empty() => ids,
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(error = %e, "mailing-list lookup failed");
                return;
            }
        };
    // Whoever was told by email of this very post is not sent it a second time.
    for (kind, notified) in told {
        let both: Vec<i64> = notified.iter().copied().filter(|n| ids.contains(n)).collect();
        if both.is_empty() {
            continue;
        }
        match bf_db::community::notifications::emailed_for_kind(pool, kind, &both).await {
            Ok(mailed) => ids.retain(|i| !mailed.contains(i)),
            Err(e) => tracing::warn!(error = %e, "mailing-list duplicate check failed"),
        }
    }
    let ids = crate::community::notifications::can_read(state, pool, node_id, ids).await;
    if ids.is_empty() {
        return;
    }
    let raw: Option<String> = sqlx::query_scalar("SELECT raw FROM posts WHERE item_id = $1 AND post_number = $2")
        .bind(topic_id)
        .bind(number)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    // The email goes to members who have not necessarily replied: hidden blocks stay out of it.
    let Some(raw) = raw.map(|r| bf_cook::strip_hidden(&r)) else { return };
    let site = state.settings().str("site_name").to_owned();
    let delay = state.settings().int("email_delay_minutes") as i32;
    let data = json!({ "by": author.username, "title": title, "url": url, "topic_id": topic_id });
    let by_locale = bf_db::community::notifications::recipients_by_locale(pool, &ids).await.unwrap_or_default();
    for (locale, group) in by_locale {
        let t = bf_core::i18n::T::new(
            locale.as_deref().unwrap_or(bf_core::i18n::site_language(state.settings().str("default_locale"))),
        );
        let vars = [("site", site.as_str()), ("title", title)];
        let subject =
            if number == 1 { t.msgf("[{site}] {title}", &vars) } else { t.msgf("Re: [{site}] {title}", &vars) };
        let wrote = t.msgf("{author} wrote:", &[("author", author.username.as_str())]);
        let read = t.msgf("Read it on the site: {link}\nReply to this email to answer in the topic.", &[("link", abs)]);
        let body = format!("{wrote}\n\n{raw}\n\n-- \n{read}\n");
        if let Err(e) = bf_db::community::notifications::deliver(
            pool,
            &state.config.base_url,
            author.id.get(),
            &group,
            "mailing_list",
            &data,
            &subject,
            &body,
            delay,
        )
        .await
        {
            tracing::warn!(error = %e, "mailing-list mail failed");
        }
    }
}

/// What people and outside systems hear about a plugin's post. Unlike `after_post` it reads no
/// mentions, so a plugin cannot ping arbitrary members, and it does not run plugin hooks: a
/// plugin's reply must not start `post.created` in plugins again (that could loop forever).
/// Webhooks and federation are told. Posts by anything else (bot, automation, federation,
/// anonymous replies) use `after_system_post`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn after_plugin_post(
    state: &AppState,
    author_id: i64,
    by: &str,
    title: &str,
    slug: &str,
    topic_id: i64,
    node_id: i64,
    number: i32,
) {
    announce(state, author_id, by, title, slug, topic_id, node_id, number, false).await;
}

/// `after_plugin_post` for a post that did not come from a plugin: plugins hear it too.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn after_system_post(
    state: &AppState,
    author_id: i64,
    by: &str,
    title: &str,
    slug: &str,
    topic_id: i64,
    node_id: i64,
    number: i32,
) {
    announce(state, author_id, by, title, slug, topic_id, node_id, number, true).await;
}

#[allow(clippy::too_many_arguments)]
async fn announce(
    state: &AppState,
    author_id: i64,
    by: &str,
    title: &str,
    slug: &str,
    topic_id: i64,
    node_id: i64,
    number: i32,
    plugins_too: bool,
) {
    let Some(pool) = &state.pool else { return };
    let url = format!("{}#post-{number}", topic_path(topic_id, slug));
    let abs = format!("{}{url}", state.config.base_url);
    let site = state.settings().str("site_name").to_owned();
    let watchers = bf_db::community::follows::watchers(pool, topic_id, node_id, author_id).await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "watcher lookup failed");
        vec![]
    });
    // Nobody hears the author and title of a post they can no longer open.
    let watchers = crate::community::notifications::can_read(state, pool, node_id, watchers).await;
    if !watchers.is_empty() {
        // The topic id lets the email be answered, and offers a link that mutes the topic.
        let data = json!({ "by": by, "title": title, "url": url, "topic_id": topic_id });
        let delay = state.settings().int("email_delay_minutes") as i32;
        let base = &state.config.base_url;
        // Each watcher reads the email in their own language; an administrator's text for this
        // mail replaces the built-in one.
        let by_locale =
            bf_db::community::notifications::recipients_by_locale(pool, &watchers).await.unwrap_or_default();
        let mut app = Vec::new();
        for (locale, ids) in by_locale {
            let locale = locale
                .unwrap_or_else(|| bf_core::i18n::site_language(state.settings().str("default_locale")).to_owned());
            let t = bf_core::i18n::T::new(&locale);
            let vars = [("site", site.as_str()), ("by", by), ("title", title), ("link", abs.as_str())];
            let (subject, body) = crate::admin::mail_templates::render(&t, "notification", &vars);
            match bf_db::community::notifications::deliver(
                pool, base, author_id, &ids, "reply", &data, &subject, &body, delay,
            )
            .await
            {
                Ok(fresh) => app.extend(fresh),
                Err(e) => tracing::error!(error = %e, "notification delivery failed"),
            }
        }
        for id in app {
            state.hub.publish(&format!("user:{id}"), "notification", data.clone());
        }
    }
    let payload = json!({ "topic_id": topic_id, "post_number": number, "title": title, "url": abs, "author": by, "node_id": node_id });
    let event = if number == 1 { "topic.created" } else { "post.created" };
    if plugins_too {
        crate::platform::plugins::dispatch(state, event, &payload);
    }
    crate::platform::webhooks::emit_external(state, event, payload).await;
    state.hub.publish(&format!("item:{topic_id}"), "post", json!({ "post_number": number, "by": by }));
    publish_activity(state, node_id, topic_id, number);
    state.microcache.clear();
}

/// Topic lists hear about a new topic (post 1) or a reply, in the forums their viewer can read.
pub(super) fn publish_activity(state: &AppState, node_id: i64, topic_id: i64, number: i32) {
    let event = if number == 1 { "topic" } else { "reply" };
    state.hub.publish(
        crate::forum::live::ACTIVITY,
        event,
        json!({ "node_id": node_id, "topic_id": topic_id, "post_number": number }),
    );
}
