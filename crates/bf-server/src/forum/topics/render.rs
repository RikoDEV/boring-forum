//! Rendering the topic page: loading the posts, the structured data, the page model and the streamed suggestions.

use crate::forum::index::topic_line;
use crate::forum::tree::guest;
use crate::http::render::layout;
use crate::http::render::page;
use crate::http::render::unread_count;
use crate::http::render::{DbResult, ago};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::{forum_path, profile_path, topic_path};
use bf_db::forum::topics::TopicRow;
use bf_db::forum::{posts, reading, topics};
use bf_web::pages::{Layout, PostView, TopicLine, TopicPage};
use chrono::{DateTime, Utc};

use super::{Tree, readable_topic, viewable_topic};

/// When a post's author last posted before it, and the previous post in the topic, if any.
type Times = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

/// Backers listed under an idea before the rest are summarised.
pub(super) const SHOWN_VOTERS: i64 = 12;

pub(super) async fn render_topic(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    slug: String,
    id: i64,
    page_n: i64,
    edit_layout: bool,
) -> Result<Response, AppError> {
    // A page number from the address: large ones would overflow the arithmetic below.
    let page_n = page_n.clamp(1, 100_000);
    let (t, tree, perms) = match viewable_topic(&state, &v, id).await {
        Ok(r) => r,
        // Merged topics keep their URL working.
        Err(AppError::NotFound) => {
            return match merged_redirect(&state, &v, id).await? {
                Some(redirect) => Ok(redirect),
                None => Err(AppError::NotFound),
            };
        }
        Err(e) => return Err(e),
    };
    // Moderators can read a deleted topic, but a merged one lives on at its target.
    if t.state == "deleted"
        && let Some(redirect) = merged_redirect(&state, &v, id).await?
    {
        return Ok(redirect);
    }
    if slug != t.slug {
        let base = topic_path(id, &t.slug);
        return Ok(
            Redirect::permanent(&if page_n > 1 { format!("{base}?page={page_n}") } else { base }).into_response()
        );
    }
    let pool = state.db()?;
    let per_page = state.settings().int("posts_per_page");
    let from = ((page_n - 1) * per_page + 1) as i32;
    let moderator = perms.has(Perms::MODERATE);
    // The author of a deleted topic sees the topic's first post, greyed, and nothing else of it.
    let own_deleted = !moderator && t.state == "deleted";
    let mode = tree.node(t.node_id).map_or("discussion", |n| n.mode.as_str()).to_owned();
    // Q&A: one page, the question, the accepted answer, then answers by votes.
    let qa = mode == "qa";
    if qa && page_n > 1 {
        return Ok(Redirect::to(&topic_path(id, &t.slug)).into_response());
    }
    let viewer_id = v.user.as_ref().map(|u| u.id);
    let mut rows = if own_deleted {
        posts::first_post(pool, id, viewer_id.map(bf_core::ids::UserId::get)).await.db()?
    } else if qa {
        posts::topic_posts_ranked(pool, id, 500, moderator, viewer_id.map(bf_core::ids::UserId::get)).await.db()?
    } else {
        posts::topic_posts(pool, id, from, per_page + 1, moderator, viewer_id.map(bf_core::ids::UserId::get))
            .await
            .db()?
    };
    // A page is a range of post numbers, so one can be empty when all its posts are hidden.
    if i64::from(from) > i64::from(t.highest_post_number) && page_n > 1 {
        return Err(AppError::NotFound);
    }
    let has_next = !qa && !own_deleted && i64::from(t.highest_post_number) >= i64::from(from) + per_page;
    let on_page = if qa {
        rows.len()
    } else {
        rows.iter().take_while(|p| i64::from(p.post_number) < i64::from(from) + per_page).count()
    };
    rows.truncate(on_page);
    prepare_rows(&state, &v, id, moderator, &mut rows).await?;
    let rows = rows;
    let base = topic_path(id, &t.slug);
    let page_url = |n: i64| if n <= 1 { base.clone() } else { format!("{base}?page={n}") };
    let node = tree.node(t.node_id).ok_or(AppError::NotFound)?;

    state.views.record(id, &v, &headers);
    if let (Some(u), Some(last)) = (&v.user, rows.last()) {
        // On the last page the member has seen all they can: numbers beyond this are posts they
        // cannot see (pending, hidden, deleted), which must not leave the topic unread for good.
        let read_to = if qa || !has_next { last.post_number.max(t.highest_post_number) } else { last.post_number };
        // Re-reading writes nothing.
        if t.last_read.is_none_or(|r| r < read_to) {
            reading::mark_read(pool, u.id.get(), id, read_to).await.db()?;
        }
    }

    let abs = |p: &str| format!("{}{p}", state.config.base_url);
    let (mut l, set) = layout(&state, &v, Some(&format!("{} - {}", t.title, node.title)));
    l.browsing = crate::community::browsing::visit(&state, &v, &headers, &format!("topic:{id}"));
    l.feed = tree.readable(&guest()).contains(&t.node_id).then(|| format!("/t/{id}/feed.rss"));
    crate::admin::themes::apply(&mut l, &v, &v.theme_headers, node.theme_id);
    l.canonical = Some(abs(&page_url(page_n)));
    l.set_paging(
        &state.config.base_url,
        (page_n > 1).then(|| page_url(page_n - 1)).as_deref(),
        has_next.then(|| page_url(page_n + 1)).as_deref(),
    );
    l.noindex = !t.listed;
    add_structured_data(&mut l, &state.config.base_url, &t, node, &rows, &base);
    if t.has_event
        && t.listed
        && let Some(ev) = bf_db::community::events::event(pool, id).await.db()?
    {
        let place = ev.location.as_deref().filter(|p| !p.trim().is_empty());
        let online = place.filter(|p| p.starts_with("https://"));
        let url = abs(&page_url(1));
        let event = bf_seo::event(&bf_seo::JsonLdEvent {
            name: &t.title,
            url: &url,
            description: (!l.description.is_empty()).then_some(l.description.as_str()),
            starts: ev.starts_at,
            ends: ev.ends_at,
            location: place.filter(|p| online.is_none_or(|o| o != *p)),
            online_url: online,
            organizer: t.author.as_deref(),
            cancelled: false,
        });
        l.json_ld.push(bf_seo::json_ld_script(&event));
    }
    l.unread_notifications = unread_count(&state, &v).await;

    let uid = v.user.as_ref().map(|u| u.id);
    let follow = t
        .follow_level
        .as_deref()
        .and_then(bf_db::community::follows::Level::parse)
        .unwrap_or(bf_db::community::follows::Level::Normal)
        .key();
    let mut shown_title = t.title.clone();
    let posts = post_views(&state, &v, &t, moderator, qa, &rows, &mut shown_title).await?;
    let (poll_html, event_html) = side_panels(&state, &v, &t, moderator, &l.csrf).await?;
    let topic_tags: Vec<(String, String)> =
        t.tag_pairs.iter().filter_map(|p| p.split_once('|')).map(|(n, s)| (n.to_owned(), s.to_owned())).collect();
    // Ideas: who backs this one, and how many votes the viewer has left to give.
    let can_vote_topic = mode == "ideas" && uid.is_some() && !t.locked && !v.actor.silenced;
    let voters = if mode == "ideas" && t.vote_count > 0 {
        bf_db::forum::topics::topic_voters(pool, id, SHOWN_VOTERS)
            .await
            .db()?
            .into_iter()
            .map(|(voter, name)| (profile_path(voter, &name), name))
            .collect()
    } else {
        vec![]
    };
    let votes_left = match &v.user {
        Some(u) if can_vote_topic => crate::forum::votes::votes_left(&state, u).await?,
        _ => None,
    };
    let p = TopicPage {
        widgets: crate::admin::themes::page_widgets(&state, "topic").await,
        can_edit_layout: v.actor.is_admin,
        editing_layout: v.actor.is_admin && edit_layout,
        layout: l,
        id,
        author_url: profile_path(t.author_id.unwrap_or(0), t.author.as_deref().unwrap_or("deleted")),
        author: t.author.clone().unwrap_or_else(|| "deleted".into()),
        started: ago(t.created_at),
        first_month: t.created_at.format("%b %Y").to_string(),
        last_ago: ago(t.last_posted_at),
        highest: t.highest_post_number,
        solved: t.solved_post_number,
        poll_html,
        poll_form: if moderator || (uid.is_some() && uid == t.author_id.map(Into::into)) {
            crate::community::polls::poll_fields(v.t(), None)
        } else {
            String::new()
        },
        event_html,
        qa,
        ideas: mode == "ideas",
        topic_votes: t.vote_count,
        voted: t.voted,
        can_vote_topic,
        votes_left,
        more_voters: (i64::from(t.vote_count) - voters.len() as i64).max(0),
        voters,
        assigned: t.assigned_to.is_some(),
        mod_actions: if moderator { bf_web::theme::mod_actions() } else { vec![] },
        can_solve: state.settings().bool("solved_enabled")
            && (moderator || (uid.is_some() && uid == t.author_id.map(Into::into))),
        base_url: base.clone(),
        tags_csv: topic_tags.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "),
        tags: topic_tags,
        can_tag: moderator || (uid.is_some() && uid == t.author_id.map(Into::into)),
        title: shown_title,
        prefix: t.prefix.clone(),
        forum_title: node.title.clone(),
        forum_url: forum_path(node.id, &node.slug),
        locked: t.locked,
        archived: t.archived,
        featured: t.featured,
        pinned: t.pinned,
        listed: t.listed,
        posts,
        prev_url: (page_n > 1).then(|| page_url(page_n - 1)),
        next_url: has_next.then(|| page_url(page_n + 1)),
        pages: if qa {
            vec![]
        } else {
            let total = (i64::from(t.highest_post_number).max(1) - 1) / per_page + 1;
            // First, last and the pages next to the current one, with a gap where some are left out.
            bf_web::pages::page_window(total, page_n, page_url)
        },
        can_reply: perms.has(Perms::REPLY) && t.state != "deleted" && ((!t.locked && !t.archived) || moderator),
        can_post_anonymously: state.settings().bool("anonymous_posting"),
        can_moderate: moderator,
        // A live topic: a moderator, or its author while nobody has replied. A deleted one: deleting it
        // again removes it for good, which only a moderator may do.
        can_delete: if t.state == "deleted" {
            moderator
        } else {
            moderator || (uid.is_some() && uid == t.author_id.map(Into::into) && t.highest_post_number <= 1)
        },
        deleted: t.state == "deleted",
        follow,
        move_targets: if moderator {
            tree.nodes
                .iter()
                .filter(|n| n.kind == "forum" && n.id != t.node_id && tree.perms(&v.actor, n.id).has(Perms::MODERATE))
                .map(|n| (n.id, n.title.clone()))
                .collect()
        } else {
            vec![]
        },
    };
    let html = bf_web::pages::render_string(&p).map_err(AppError::internal)?;
    let res = stream_with_suggestions(&state, &v, &tree, &html, id, has_next)?;
    Ok(page(res, set, v.user.is_none()))
}

/// Where a merged topic went, if the viewer may read the target. Only then, or the redirect
/// would give away the target's title.
async fn merged_redirect(state: &AppState, v: &Viewer, id: i64) -> Result<Option<Response>, AppError> {
    let Some(into) = bf_db::moderation::topics::merged_into(state.db()?, id).await.db()? else {
        return Ok(None);
    };
    let (target, _, _) = readable_topic(state, v, into).await?;
    Ok(Some(Redirect::permanent(&topic_path(target.id, &target.slug)).into_response()))
}

/// Description, `DiscussionForumPosting`/`QAPage` and breadcrumb data for search engines.
fn add_structured_data(
    l: &mut Layout,
    base_url: &str,
    t: &TopicRow,
    node: &bf_db::forum::nodes::Node,
    rows: &[posts::PostRow],
    base: &str,
) {
    let abs = |p: &str| format!("{base_url}{p}");
    if let Some(first) = rows.iter().find(|p| p.post_number == 1) {
        l.description = bf_seo::excerpt(&first.cooked, 160);
        // (url, author, author_url, date, text) per post; JsonLdPost borrows from these.
        let meta: Vec<_> = rows
            .iter()
            .filter(|p| p.action.is_none() && p.state == "visible")
            .map(|p| {
                let a = p.author.clone().unwrap_or_else(|| "deleted".into());
                let aurl = abs(&profile_path(p.author_id.unwrap_or(0), &a));
                (abs(&format!("{base}#post-{}", p.post_number)), a, aurl, p.created_at, bf_seo::excerpt(&p.cooked, 500))
            })
            .collect();
        let ld: Vec<_> = meta
            .iter()
            .map(|(url, a, aurl, date, text)| bf_seo::JsonLdPost {
                author: a,
                author_url: aurl,
                date: *date,
                text: text.clone(),
                url,
            })
            .collect();
        if let Some((lead, replies)) = ld.split_first() {
            let answer_url = t.solved_post_number.map(|n| abs(&format!("{base}#post-{n}")));
            let answer = replies.iter().find(|r| Some(r.url) == answer_url.as_deref());
            let posting = match answer {
                Some(a) => bf_seo::qa_page(&t.title, lead, a, replies, t.reply_count.into()),
                None => bf_seo::discussion_posting(&t.title, lead, replies, t.reply_count.into()),
            };
            l.json_ld.push(bf_seo::json_ld_script(&posting));
        }
    }
    l.json_ld.push(bf_seo::json_ld_script(&bf_seo::breadcrumbs(&[
        ("Forums", &abs("/")),
        (&node.title, &abs(&forum_path(node.id, &node.slug))),
        (&t.title, &abs(base)),
    ])));
}

/// The poll, event and live-topic panels. Only topics that have one pay for it, so other pages
/// stay within their query budget.
async fn side_panels(
    state: &AppState,
    v: &Viewer,
    t: &TopicRow,
    moderator: bool,
    csrf: &str,
) -> Result<(String, String), AppError> {
    let id = t.id;
    let uid = v.user.as_ref().map(|u| u.id);
    let poll_html = match &t.poll_question {
        Some(_) => {
            let manage = moderator || (uid.is_some() && uid == t.author_id.map(Into::into));
            let voter = v.user.as_ref().map(|u| crate::community::polls::Voter {
                id: u.id.get(),
                groups: v.actor.groups.iter().map(|g| g.0).collect(),
                trust_level: u.trust_level,
            });
            crate::community::polls::poll_html(state, id, voter.as_ref(), csrf, t.locked, moderator, manage, v.t())
                .await?
        }
        None => String::new(),
    };
    let mut event_html = if t.has_event {
        let manage = moderator || (uid.is_some() && uid == t.author_id.map(Into::into));
        crate::community::events::event_html(state, id, uid.map(bf_core::ids::UserId::get), csrf, manage, v.t()).await?
    } else {
        String::new()
    };
    // Live topics: the panel, or for those who may start one, the form to do so.
    if t.has_live
        || ((moderator || (uid.is_some() && uid == t.author_id.map(Into::into)))
            && state.settings().bool("live_topics_enabled"))
    {
        event_html.push_str(&crate::forum::live_topics::live_html(state, v, id, t.node_id, t.author_id, csrf).await?);
    }
    Ok((poll_html, event_html))
}

/// The page streamed in two parts: everything up to the suggestions goes out at once, because
/// the suggestions' query must not hold up the posts.
fn stream_with_suggestions(
    state: &AppState,
    v: &Viewer,
    tree: &Tree,
    html: &str,
    id: i64,
    has_next: bool,
) -> Result<Response, AppError> {
    let (head, tail) = html.split_once(bf_web::pages::SUGGESTED_MARKER).unwrap_or((html, ""));
    let (head, tail) = (head.to_owned(), tail.to_owned());
    let uid = v.user.as_ref().map(|u| u.id);
    let suggest_from = tree.readable(&v.actor);
    let forums: std::collections::HashMap<i64, (String, String)> =
        tree.nodes.iter().map(|n| (n.id, (n.title.clone(), forum_path(n.id, &n.slug)))).collect();
    let (pool2, locale, settings) = (state.db()?.clone(), v.locale, state.settings());
    let rest = async move {
        let mut out = String::new();
        if !has_next {
            match topics::latest_topics(&pool2, &suggest_from, uid.map(bf_core::ids::UserId::get), false, 6).await {
                Ok(others) => {
                    let mut topics: Vec<TopicLine> = others
                        .iter()
                        .filter(|o| o.id != id)
                        .take(5)
                        .map(|o| TopicLine { forum: forums.get(&o.node_id).cloned(), ..topic_line(o) })
                        .collect();
                    // Unread first.
                    topics.sort_by_key(|l| l.unread.is_none());
                    if !topics.is_empty() {
                        let mut layout = Layout::new(&settings, None);
                        layout.t = bf_core::i18n::T::new(locale);
                        match bf_web::pages::render_string(&bf_web::pages::SuggestedPart { layout, topics }) {
                            Ok(part) => out = part,
                            Err(e) => tracing::error!(error = %e, "suggested topics render failed"),
                        }
                    }
                }
                // The page is already on its way, so this part is just left out.
                Err(e) => tracing::error!(error = %e, "suggested topics failed"),
            }
        }
        out.push_str(&tail);
        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(out))
    };
    let first = futures_util::stream::once(std::future::ready(Ok::<_, std::convert::Infallible>(
        axum::body::Bytes::from(head),
    )));
    let stream = futures_util::StreamExt::chain(first, futures_util::stream::once(rest));
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], axum::body::Body::from_stream(stream)).into_response())
}

/// A topic silent this long gets a "3 weeks later" marker before the next post.
pub(super) const TIME_GAP_DAYS: i64 = 7;

/// A member away this long is welcomed back with a note on their post.
pub(super) const RETURNING_DAYS: i64 = 120;

/// "3 days", "2 months", "1 year" in the reader's language.
pub(super) fn time_span(t: &bf_core::i18n::T, days: i64) -> String {
    match days {
        d if d < 30 => t.n("time.days", d),
        d if d < 365 => t.n("time.months", d / 30),
        d => t.n("time.years", d / 365),
    }
}

/// An action entry's sentence and any detail. Stored as `locked`, or `locked: a reason`.
pub(super) fn action_sentence(t: &bf_core::i18n::T, stored: &str) -> (String, Option<String>) {
    let (action, rest) = stored.split_once(": ").map_or((stored, None), |(a, r)| (a, Some(r.to_owned())));
    let key = format!("action.{action}");
    let text = t.t(&key);
    // An action this version has no words for still reads as something.
    (if text == key { action.to_owned() } else { text }, rest)
}

/// Reveals `[hide]` blocks to those allowed to see them, and scopes footnote ids to their post.
pub(super) async fn prepare_rows(
    state: &AppState,
    v: &Viewer,
    id: i64,
    moderator: bool,
    rows: &mut [posts::PostRow],
) -> Result<(), AppError> {
    let pool = state.db()?;
    // [hide] blocks: for moderators, the post's author, and anyone who has posted in the topic.
    if rows.iter().any(|p| p.cooked.contains(bf_cook::HIDDEN_MARKER)) {
        let viewer = v.user.as_ref().map(|u| u.id);
        let replied = match viewer {
            Some(u) => bf_db::community::onboarding::has_posted(pool, id, u.get()).await.db()?,
            None => false,
        };
        for p in rows.iter_mut().filter(|p| p.cooked.contains(bf_cook::HIDDEN_MARKER)) {
            if (moderator || replied || (viewer.is_some() && p.author_id == viewer.map(bf_core::ids::UserId::get)))
                && let Some(source) = posts::post_ref(pool, p.id).await.db()?
            {
                p.cooked = bf_cook::reveal_hidden(&p.cooked, &source.raw);
            }
        }
    }
    // Each post is cooked alone, so its footnote ids are only unique once scoped to the post.
    for p in rows.iter_mut().filter(|p| p.cooked.contains("id=\"fn")) {
        let scope = format!("p{}-", p.id);
        p.cooked = p
            .cooked
            .replace("#fn-", &format!("#{scope}fn-"))
            .replace("#fnref-", &format!("#{scope}fnref-"))
            .replace("id=\"fn-", &format!("id=\"{scope}fn-"))
            .replace("id=\"fnref-", &format!("id=\"{scope}fnref-"));
    }
    Ok(())
}

/// The posts as the topic page shows them to this viewer, translated into their language when
/// someone wrote a translation (which may change `shown_title` too).
#[allow(clippy::too_many_arguments)]
pub(super) async fn post_views(
    state: &AppState,
    v: &Viewer,
    t: &TopicRow,
    moderator: bool,
    qa: bool,
    rows: &[posts::PostRow],
    shown_title: &mut String,
) -> Result<Vec<PostView>, AppError> {
    let pool = state.db()?;
    let uid = v.user.as_ref().map(|u| u.id);
    let see_deleted =
        moderator || (t.state == "deleted" && uid.is_some() && t.author_id == uid.map(bf_core::ids::UserId::get));
    let t_lang = bf_core::i18n::T::new(v.locale);
    let show_levels = state.settings().bool("show_trust_levels");
    let rank_spec = state.settings().str("rank_names").to_owned();
    let post_ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let timeline: std::collections::HashMap<i64, Times> =
        posts::post_timeline(pool, &post_ids).await.db()?.into_iter().map(|(id, a, b)| (id, (a, b))).collect();
    // Staff see the notes on these posts; nobody else is sent them or asked for them.
    let mut notes: std::collections::HashMap<i64, Vec<bf_web::pages::StaffNote>> = std::collections::HashMap::new();
    if moderator {
        for n in bf_db::moderation::topics::post_notes(pool, &post_ids).await.db()? {
            notes.entry(n.post_id).or_default().push(bf_web::pages::StaffNote {
                id: n.id,
                author: n.author.unwrap_or_else(|| "deleted".into()),
                when: ago(n.created_at),
                body: n.body,
            });
        }
    }
    // Links in posts are followed or not by the site's settings, when the page is built.
    let nofollow = crate::forum::nofollow::Policy::from_settings(&state.settings(), &state.config.base_url);
    let mut posts = rows
        .iter()
        .map(|p| {
            let author = p.author.clone().unwrap_or_else(|| "deleted".into());
            PostView {
                staff_notes: notes.remove(&p.id).unwrap_or_default(),
                replies: p.reply_first.map(|first| (p.reply_total, first)),
                deleted: p.state == "deleted",
                id: p.id,
                number: p.post_number,
                author_url: profile_path(p.author_id.unwrap_or(0), &author),
                author,
                created: ago(p.created_at),
                created_iso: p.created_at.to_rfc3339(),
                // Moderators see hidden posts in full. A deleted one is shown (greyed) to staff and to the
                // author of a deleted topic, and is a placeholder for anyone else.
                cooked: if p.state == "visible" || p.state == "hidden" || (p.state == "deleted" && see_deleted) {
                    nofollow.apply(&p.cooked, p.author_trust_level.map(i64::from)).into_owned()
                } else {
                    format!("<p><em>[{}]</em></p>", p.state)
                },
                hidden_reason: (p.state == "hidden").then(|| p.hidden_reason.clone().unwrap_or_default()),
                reply_to: p.reply_to_number.map(|n| (n, p.reply_to_author.clone().unwrap_or_else(|| "deleted".into()))),
                time_gap: time_gap(&t_lang, p.created_at, timeline.get(&p.id)),
                author_note: author_note(&t_lang, p, timeline.get(&p.id)),
                action: p.action.as_deref().map(|a| action_sentence(&t_lang, a).0),
                action_detail: p.action.as_deref().and_then(|a| action_sentence(&t_lang, a).1),
                edited: p.edit_count > 0,
                can_edit: p.state != "deleted"
                    && (moderator
                        || (uid.is_some()
                            && p.author_id == uid.map(bf_core::ids::UserId::get)
                            && !t.locked
                            && !v.actor.silenced)
                        || (p.wiki
                            && !t.locked
                            && !v.actor.silenced
                            && v.user.as_ref().is_some_and(|u| {
                                i64::from(u.trust_level) >= state.settings().int("wiki_min_trust_level")
                            }))),
                wiki: p.wiki,
                signature: p.author_signature.clone().filter(|_| state.settings().bool("signatures_enabled")),
                ignored: p.ignored,
                likes: p.like_count,
                reaction_summary: p.reaction_summary.clone(),
                reaction_pills: reaction_pills(p),
                my_reaction: p.my_reaction.unwrap_or(0),
                author_reputation: p.author_reputation.unwrap_or(0),
                author_posts: p.author_posts.unwrap_or(0),
                author_rank: p.author_reputation.and_then(|rep| {
                    let name = crate::forum::topics::rank_name(&rank_spec, rep);
                    let (_, _, tier, colour) = crate::forum::topics::rank_step(&rank_spec, rep);
                    (!name.is_empty()).then_some((name, tier, colour))
                }),
                author_trust: p
                    .author_trust_level
                    .filter(|_| show_levels)
                    .map(|l| (l, crate::community::trust::level_label(state, &t_lang, l))),
                author_group: p.author_group.clone().unwrap_or_default(),
                author_special: p.author_special.unwrap_or(false),
                author_group_colour: p.author_group_colour.clone(),
                author_group_icon: p.author_group_icon.clone(),
                author_group_image: p.author_group_image.clone(),
                author_is_op: p.post_number > 1 && p.author_id.is_some() && p.author_id == t.author_id,
                bookmarked: p.bookmarked,
                liked: p.liked,
                can_like: uid.is_some()
                    && p.author_id != uid.map(bf_core::ids::UserId::get)
                    && p.action.is_none()
                    && p.state == "visible",
                vote_score: p.vote_score,
                my_vote: p.my_vote.unwrap_or(0),
                // Answers, not the question, and not your own.
                can_vote: qa
                    && uid.is_some()
                    && p.post_number > 1
                    && p.author_id != uid.map(bf_core::ids::UserId::get)
                    && p.state == "visible"
                    && !t.locked
                    && !v.actor.silenced,
                show_votes: qa && p.post_number > 1,
            }
        })
        .collect::<Vec<_>>();
    // Content translation: the reader's language, when someone wrote it.
    crate::forum::translations::apply_posts(state, v.locale, &mut posts, shown_title).await;
    Ok(posts)
}

/// "3 weeks later" before a post that follows a long silence in the topic.
fn time_gap(t_lang: &bf_core::i18n::T, created_at: DateTime<Utc>, times: Option<&Times>) -> Option<String> {
    times
        .and_then(|(_, before)| *before)
        .map(|before| (created_at - before).num_days())
        .filter(|days| *days >= TIME_GAP_DAYS)
        .map(|days| t_lang.f("time.later", &[("span", &time_span(t_lang, days))]))
}

/// A note on the author's first post, or on a return after a long absence.
fn author_note(t_lang: &bf_core::i18n::T, p: &posts::PostRow, times: Option<&Times>) -> Option<String> {
    match (p.author_id, p.action.as_ref(), times) {
        (Some(_), None, Some((None, _))) => Some(t_lang.t("action.first_post")),
        (Some(_), None, Some((Some(last), _))) if (p.created_at - *last).num_days() >= RETURNING_DAYS => {
            Some(t_lang.f("action.returned", &[("span", &time_span(t_lang, (p.created_at - *last).num_days()))]))
        }
        _ => None,
    }
}

/// (icon, count, whether it is the viewer's own) for each reaction on a post.
fn reaction_pills(p: &posts::PostRow) -> Vec<(String, String, bool)> {
    p.reaction_summary
        .split(" · ")
        .filter_map(|x| x.split_once(' '))
        .map(|(icon, n)| {
            let mine = bf_web::theme::reactions().iter().any(|r| r.2 == icon && r.0 == p.my_reaction.unwrap_or(0));
            (icon.to_owned(), n.to_owned(), mine)
        })
        .collect()
}
