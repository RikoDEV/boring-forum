//! Member profiles, group pages, hover cards and member summaries.

use crate::forum::topics::rank_name;
use crate::forum::topics::rank_step;
use crate::forum::topics::readable_topic;
use crate::http::render::{DbResult, unread_count};
use crate::http::render::{ago, layout, page};
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::{profile_path, topic_path};
use bf_db::accounts::users;
use bf_db::community::{groups, visitors};
use bf_db::forum::bookmarks;
use bf_web::pages::{
    ActivityView, GroupPage, ProfilePage, StatusReplyView, StatusView, WarningBox, WarningView, render,
};
use serde::Deserialize;
use serde_json::json;

pub fn cached_pages() -> Router<AppState> {
    Router::new()
        .route("/u/{name}", get(profile))
        .route("/g/{slug}", get(group_page))
        .route("/api/v1/users/{name}/card", get(user_card))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/u/{id}/summary", get(user_summary))
        .route("/settings/visitors", post(set_visitors))
        .route("/t/{id}/title", post(change_title))
}

/// Whether this viewer may see a member's profile, and whether it is their own and whether they
/// are staff. Staff always see profiles: they need them to moderate. The page and its JSON
/// twin share this check.
pub(crate) async fn profile_access(
    pool: &sqlx::PgPool,
    v: &Viewer,
    id: bf_core::ids::UserId,
    visibility: &str,
) -> Result<(bool, bool), AppError> {
    let own = v.user.as_ref().is_some_and(|u| u.id == id);
    let staff_view = is_staff(v);
    let visible = match visibility {
        "nobody" => own || staff_view,
        "members" => v.user.is_some(),
        _ => true,
    };
    if !visible {
        return Err(AppError::Forbidden);
    }
    // An account awaiting approval is not part of the community yet: only it and staff see it.
    if !own && !staff_view && !users::is_approved(pool, id.get()).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok((own, staff_view))
}

#[derive(Deserialize)]
struct ProfileQuery {
    #[serde(default)]
    tab: String,
}

async fn profile(
    State(state): State<AppState>,
    v: Viewer,
    Path(name): Path<String>,
    Query(pq): Query<ProfileQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let Some(id) = users::user_id_by_name(pool, &name).await.db()? else {
        return moved_profile_redirect(pool, &v, &name, &pq.tab).await;
    };
    let p = users::profile(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let tree = state.forum.get(pool).await?;
    let (own, staff_view) = profile_access(pool, &v, id, &p.profile_visibility).await?;
    // A page the browser only loads ahead of a possible click is not a visit.
    let speculative = headers.get("sec-purpose").is_some();
    let visitor = v.user.as_ref().map(|u| u.id).filter(|u| *u != id);
    if !speculative && (visitor.is_some() || v.user.is_none()) {
        record_visit(pool, id.get(), visitor.map(bf_core::ids::UserId::get), pq.tab.is_empty()).await;
    }
    // Members and staff only, like the warning history it links to.
    let warning_box = if own || staff_view { Some(warning_box(pool, id.get()).await?) } else { None };
    let followers_visible = !p.hide_followers || own || staff_view;
    let (follower_count, follower_names) = if followers_visible {
        let limit = if pq.tab == "followers" { 100 } else { 12 };
        bf_db::community::follows::followers(pool, id.get(), limit).await.db()?
    } else {
        (0, vec![])
    };
    let following = match visitor {
        Some(me) => Some(bf_db::community::follows::is_following(pool, me.get(), id.get()).await.db()?),
        None => None,
    };
    let recent_visitors = if own && p.show_visitors {
        bf_db::community::visitors::visitors(pool, id.get())
            .await
            .db()?
            .into_iter()
            .take(10)
            .map(|(name, when)| (name, ago(when)))
            .collect()
    } else {
        vec![]
    };
    // The Votes tab, where the site has an ideas forum.
    let has_ideas = tree.nodes.iter().any(|n| n.mode == "ideas");
    let tab: &'static str = match pq.tab.as_str() {
        "topics" => "topics",
        "status" => "status",
        "followers" if !p.hide_followers || own || staff_view => "followers",
        "summary" => "summary",
        "badges" => "badges",
        "votes" if has_ideas => "votes",
        "drafts" if own => "drafts",
        "bookmarks" if own => "bookmarks",
        _ => "posts",
    };
    let readable = tree.readable(&v.actor);
    let posts = if tab == "posts" { users::user_posts(pool, id, &readable, 20).await.db()? } else { vec![] };
    let bookmark_rows =
        if tab == "bookmarks" { bookmarks::bookmarks(pool, id.get(), &readable).await.db()? } else { vec![] };
    let vote_rows = if tab == "votes" {
        bf_db::forum::topics::voted_topics(pool, id.get(), &readable, 50).await.db()?
    } else {
        vec![]
    };
    let topic_rows = if tab == "topics" { users::user_topics(pool, id, &readable, 30).await.db()? } else { vec![] };
    let statuses = if tab == "status" { status_views(pool, &v, id.get()).await? } else { vec![] };
    let top_posts = if tab == "summary" {
        bf_db::forum::summary::top_posts_of(pool, id.get(), &readable)
            .await
            .db()?
            .into_iter()
            .map(|(tid, slug, title, n, likes)| {
                (format!("{}#post-{n}", topic_path(tid, &slug)), title, likes.to_string())
            })
            .collect()
    } else {
        vec![]
    };
    let (badges, badge_points) = if tab == "badges" {
        let t = v.t();
        let (list, points) = bf_db::community::badges::badges_of(pool, id.get()).await.db()?;
        let list = list
            .into_iter()
            .map(|(name, about, tier, icon, color, level)| (t.msg(&name), t.msg(&about), tier, icon, color, level))
            .collect();
        (list, points)
    } else {
        (vec![], 0)
    };
    let drafts = if tab == "drafts" { draft_rows(pool, id.get()).await? } else { vec![] };
    let (mut l, set) = layout(&state, &v, Some(&p.username));
    l.canonical = Some(format!("{}{}", state.config.base_url, profile_path(p.id.get(), &p.username)));
    // Profiles without posts are thin content.
    l.noindex = p.post_count == 0 || p.profile_visibility != "everyone";
    if !l.noindex && pq.tab.is_empty() {
        let base = &state.config.base_url;
        let url = format!("{base}{}", profile_path(p.id.get(), &p.username));
        let image = format!("{base}/avatar/{}.svg", p.username);
        let about = bf_seo::excerpt(p.about_html.as_deref().unwrap_or_default(), 300);
        l.json_ld.push(bf_seo::json_ld_script(&bf_seo::profile_page(&bf_seo::JsonLdProfile {
            name: p.display_name.as_deref().filter(|n| !n.is_empty()).unwrap_or(&p.username),
            url: &url,
            image: Some(&image),
            joined: p.joined_at,
            about: Some(about.as_str()).filter(|a| !a.is_empty()),
            post_count: p.post_count.into(),
        })));
    }
    l.unread_notifications = unread_count(&state, &v).await;
    let ignore = match &v.user {
        Some(u) if u.id != p.id => {
            Some(bf_db::community::follows::ignored_ids(pool, u.id.get()).await.db()?.contains(&p.id.get()))
        }
        _ => None,
    };
    let fields = public_fields(pool, p.id.get()).await?;
    let staff_notes = if is_staff(&v) {
        Some(
            bf_db::moderation::topics::notes(pool, p.id.get())
                .await
                .db()?
                .into_iter()
                .map(|n| (n.author.unwrap_or_else(|| "deleted".into()), ago(n.created_at), n.body))
                .collect(),
        )
    } else {
        None
    };
    let page_body = ProfilePage {
        layout: l,
        profile_views: p.profile_views,
        warning_box,
        followers_visible,
        hide_followers: p.hide_followers,
        follower_count,
        follower_names,
        following,
        keeps_visitors: p.show_visitors,
        recent_visitors,
        statuses,
        can_post_status: own && !v.actor.silenced,
        top_posts,
        badges,
        badge_points,
        id: p.id.get(),
        display_name: p.display_name.clone(),
        cover_url: p.cover_key.as_ref().map(|k| format!("/uploads/{k}")),
        cover_y: p.cover_y,
        group_colour: p.group_colour.clone().and_then(safe_colour),
        group_icon: p.group_icon.clone(),
        group_image: p.group_image.clone().filter(|i| bf_core::settings::upload_path_ok(i)),
        likes_received: p.likes_received,
        reputation: p.reputation,
        solved_count: p.solved_count,
        rank: rank_name(state.settings().str("rank_names"), p.reputation),
        trust: (state.settings().bool("show_trust_levels") || own || staff_view)
            .then(|| (p.trust_level, crate::community::trust::level_label(&state, &v.t(), p.trust_level))),
        rank_step: rank_step(state.settings().str("rank_names"), p.reputation),
        recent_badges: bf_db::community::badges::recent_badges(pool, p.id.get(), 5)
            .await
            .db()?
            .into_iter()
            .map(|(id, name, tier, icon, color, level, rare)| (id, v.t().msg(&name), tier, icon, color, level, rare))
            .collect(),
        status_text: p.status_text.clone(),
        records_visitors: p.show_visitors && !own && v.user.is_some(),
        tab,
        own,
        drafts,
        fields,
        staff_notes,
        location: p.location,
        website: p.website,
        ignore,
        username: p.username,
        title: p.title,
        group: p.group_name,
        extra_groups: extra_groups(pool, p.id.get()).await?,
        about_html: p.about_html,
        post_count: p.post_count,
        joined: bf_web::ago::Ago::day(p.joined_at),
        last_seen: p.last_seen_at.filter(|_| !p.hide_last_seen || own || is_staff(&v)).map(ago),
        has_ideas,
        activity: activity_feed(vote_rows, bookmark_rows, topic_rows, posts),
    };
    Ok(page(render(&page_body), set, v.user.is_none()))
}

/// The tab's rows as one list: only the tab being shown has any, the others are empty.
fn activity_feed(
    votes: Vec<bf_db::forum::topics::VotedTopic>,
    bookmarks: Vec<(i64, String, String, i32, chrono::DateTime<chrono::Utc>)>,
    topics: Vec<(i64, String, String, i32, chrono::DateTime<chrono::Utc>)>,
    posts: Vec<users::ActivityRow>,
) -> Vec<ActivityView> {
    votes
        .into_iter()
        .map(|t| ActivityView {
            url: topic_path(t.id, &t.slug),
            title: t.title,
            excerpt: format!(
                "{} {}{}",
                t.vote_count,
                if t.vote_count == 1 { "vote" } else { "votes" },
                if t.locked { " · closed" } else { "" }
            ),
            when: ago(t.voted_at),
        })
        .chain(
            bookmarks
                .into_iter()
                .map(|(item, slug, title, n, when)| ActivityView {
                    url: format!("{}#post-{n}", topic_path(item, &slug)),
                    title,
                    excerpt: format!("Post #{n}"),
                    when: ago(when),
                })
                .chain(
                    topics
                        .into_iter()
                        .map(|(tid, slug, title, replies, created)| ActivityView {
                            url: topic_path(tid, &slug),
                            title,
                            excerpt: format!("{replies} replies"),
                            when: ago(created),
                        })
                        .chain(posts.into_iter().map(|a| ActivityView {
                            url: format!("{}#post-{}", topic_path(a.item_id, &a.slug), a.post_number),
                            title: a.title,
                            excerpt: bf_seo::excerpt(&a.cooked, 200),
                            when: ago(a.created_at),
                        })),
                ),
        )
        .collect()
}

/// A renamed member's old address follows them; `@group` mentions link here too.
async fn moved_profile_redirect(pool: &sqlx::PgPool, v: &Viewer, name: &str, tab: &str) -> Result<Response, AppError> {
    if let Some(id) = bf_db::accounts::users::user_by_old_name(pool, name).await.db()? {
        let current = users::profile(pool, id).await.db()?.ok_or(AppError::NotFound)?;
        // An old name must not tell who a private profile belongs to now.
        profile_access(pool, v, id, &current.profile_visibility).await?;
        let keep = if tab.is_empty() { String::new() } else { format!("?tab={tab}") };
        return Ok(Redirect::permanent(&format!("{}{keep}", profile_path(id.get(), &current.username))).into_response());
    }
    groups::group_page(pool, name, v.user.as_ref().map(|u| u.id.get())).await.db()?.ok_or(AppError::NotFound)?;
    Ok(Redirect::to(&format!("/g/{name}")).into_response())
}

/// Records who looked and counts the view. Failures are logged: a page must load without them.
async fn record_visit(pool: &sqlx::PgPool, profile: i64, visitor: Option<i64>, main_tab: bool) {
    if let Some(viewer) = visitor
        && let Err(e) = bf_db::community::visitors::record_visit(pool, profile, viewer).await
    {
        tracing::warn!(error = %e, "visit not recorded");
    }
    // Tabs of the same profile are not new views.
    if main_tab && let Err(e) = bf_db::community::visitors::count_profile_view(pool, profile).await {
        tracing::warn!(error = %e, "view not counted");
    }
}

/// Warning points and the latest warnings, for the member and staff.
async fn warning_box(pool: &sqlx::PgPool, id: i64) -> Result<WarningBox, AppError> {
    let (points, silenced, suspended) = bf_db::moderation::warnings::warning_state(pool, id).await.db()?;
    let now = chrono::Utc::now();
    let recent = bf_db::moderation::warnings::warnings_of(pool, id)
        .await
        .db()?
        .into_iter()
        .take(3)
        .map(|w| WarningView {
            points: w.points,
            reason: w.reason,
            by: w.moderator.unwrap_or_else(|| "staff".into()),
            when: ago(w.created_at),
            active: w.expires_at > now,
        })
        .collect();
    Ok(WarningBox { points, silenced, suspended, recent })
}

/// The Status tab: each status with its replies, and what this viewer may do to them.
async fn status_views(pool: &sqlx::PgPool, v: &Viewer, id: i64) -> Result<Vec<StatusView>, AppError> {
    let (rows, mut replies) = bf_db::community::statuses::feed(pool, id, 30).await.db()?;
    let me = v.user.as_ref().map(|u| u.id);
    let staff = is_staff(v);
    Ok(rows
        .into_iter()
        .map(|s| {
            let manage = staff || me == Some(s.user_id.into());
            let (mine, rest): (Vec<_>, Vec<_>) =
                std::mem::take(&mut replies).into_iter().partition(|r| r.status_id == s.id);
            replies = rest;
            StatusView {
                id: s.id,
                when: ago(s.created_at),
                can_manage: manage,
                can_reply: me.is_some() && !v.actor.silenced && (!s.locked || manage),
                locked: s.locked,
                replies: mine
                    .into_iter()
                    .map(|r| StatusReplyView {
                        id: r.id,
                        author_url: profile_path(r.author_id, &r.author),
                        can_delete: manage || me == Some(r.author_id.into()),
                        author: r.author,
                        body: r.body,
                        when: ago(r.created_at),
                    })
                    .collect(),
                body: s.body,
            }
        })
        .collect())
}

/// The Drafts tab: unsent topics and replies, each with a link back into the composer.
async fn draft_rows(pool: &sqlx::PgPool, id: i64) -> Result<Vec<(String, String, bf_web::ago::Ago)>, AppError> {
    Ok(bf_db::forum::uploads::drafts(pool, id)
        .await
        .db()?
        .into_iter()
        .filter_map(|(ctx, data, when)| {
            let (kind, n) = ctx.split_once(':')?;
            let n: i64 = n.parse().ok()?;
            let title = data.get("title").and_then(|t| t.as_str()).filter(|t| !t.is_empty());
            let (label, url) = match kind {
                "new-topic" => {
                    (title.map_or_else(|| "New topic".to_owned(), str::to_owned), format!("/c/x/{n}#new-topic"))
                }
                _ => ("Reply".to_owned(), format!("/t/x/{n}/unread")),
            };
            Some((label, url, ago(when)))
        })
        .collect())
}

/// A colour only when it is a safe hex value.
fn safe_colour(c: String) -> Option<String> {
    (c.len() <= 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())).then_some(c)
}

/// The member's other groups, with a colour only if it is a safe hex value.
async fn extra_groups(pool: &sqlx::PgPool, user: i64) -> Result<Vec<(String, Option<String>, String)>, AppError> {
    Ok(groups::secondary_groups(pool, user)
        .await
        .db()?
        .into_iter()
        .map(|(n, c, s)| (n, c.and_then(safe_colour), s))
        .collect())
}

/// The profile fields marked public that this member filled in.
async fn public_fields(pool: &sqlx::PgPool, user: i64) -> Result<Vec<bf_web::pages::ProfileFieldView>, AppError> {
    let all_fields = bf_db::accounts::fields::fields(pool).await.db()?;
    let values = bf_db::accounts::fields::values(pool, user.into()).await.db()?;
    Ok(all_fields
        .iter()
        .filter(|f| f.public)
        .filter_map(|f| {
            let value = values.iter().find(|(id, _)| *id == f.id).map(|(_, v)| v.clone())?;
            // Only what still looks like a safe link becomes one; older values stay text.
            let link = f.kind == "url"
                && (value.starts_with("https://") || value.starts_with("http://"))
                && !value.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`'));
            Some(bf_web::pages::ProfileFieldView { label: f.label.clone(), value, icon: f.icon.clone(), link })
        })
        .collect())
}

/// `@mention` links point here (bf-cook knows names, not ids).
async fn group_page(
    State(state): State<AppState>,
    v: Viewer,
    Path(slug): Path<String>,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    let viewer = v.user.as_ref().map(|u| u.id.get());
    let (name, members) = groups::group_page(state.db()?, &slug, viewer).await.db()?.ok_or(AppError::NotFound)?;
    let (mut l, set) = layout(&state, &v, Some(&name));
    l.area = crate::admin::themes::page_area(&state, &v, "groups", edit).await;
    Ok(page(render(&GroupPage { layout: l, slug, name, members }), set, v.user.is_none()))
}

/// `GET /api/v1/users/{name}/card` for hover cards.
/// The card that opens over a member's name: who they are, how active, and the follow and
/// message buttons. The same visibility rules as the profile page.
async fn user_card(State(state): State<AppState>, v: Viewer, Path(name): Path<String>) -> Result<Response, AppError> {
    let pool = state.db()?;
    let id = users::user_id_by_name(pool, &name).await.db()?.ok_or(AppError::NotFound)?;
    let p = users::profile(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (own, staff) = profile_access(pool, &v, id, &p.profile_visibility)
        .await
        .map_err(|e| if matches!(e, AppError::Forbidden) { AppError::NotFound } else { e })?;
    let seen = p.last_seen_at.filter(|_| !p.hide_last_seen || own || staff);
    let online = seen.is_some_and(|t| chrono::Utc::now() - t < chrono::Duration::minutes(15));
    let followers = if !p.hide_followers || own || staff {
        Some(bf_db::community::follows::followers(pool, id.get(), 0).await.db()?.0)
    } else {
        None
    };
    let following = match v.user.as_ref().filter(|_| !own) {
        Some(me) => Some(bf_db::community::follows::is_following(pool, me.id.get(), id.get()).await.db()?),
        None => None,
    };
    let badges: Vec<_> = bf_db::community::badges::recent_badges(pool, id.get(), 4)
        .await
        .db()?
        .into_iter()
        .map(|(_, name, tier, icon, color, level, rare)| {
            json!({ "name": v.t().msg(&name), "tier": tier, "icon": icon, "color": color, "level": level, "rare": rare })
        })
        .collect();
    let colour = p.group_colour.and_then(safe_colour);
    let step = rank_step(state.settings().str("rank_names"), p.reputation);
    let bio = p.about_html.as_deref().map(|h| bf_seo::excerpt(h, 180)).filter(|b| !b.is_empty());
    let rank = rank_name(state.settings().str("rank_names"), p.reputation);
    let mut card = json!({
        "id": p.id,
        "username": p.username,
        "display_name": p.display_name,
        "title": p.title,
        "url": profile_path(p.id.get(), &p.username),
        "group": p.group_name,
        "group_colour": colour,
        "groups": extra_groups(pool, p.id.get()).await.unwrap_or_default().into_iter().map(|(name, colour, slug)| json!({ "name": name, "colour": colour, "slug": slug })).collect::<Vec<_>>(),
        "rank": Some(rank).filter(|r| !r.is_empty()),
        "rank_tier": step.2,
        "rank_color": step.3,
        "trust": (state.settings().bool("show_trust_levels") || own || staff)
            .then(|| json!({ "level": p.trust_level, "name": crate::community::trust::level_label(&state, &v.t(), p.trust_level) })),
        "cover": p.cover_key.as_ref().map(|k| format!("/uploads/{k}")),
        "cover_y": p.cover_y,
        "status": p.status_text,
        "bio": bio,
        "location": p.location.filter(|l| !l.is_empty()),
        "joined": p.joined_at.format("%b %Y").to_string(),
        "joined_at": p.joined_at.to_rfc3339(),
        "last_seen": seen.map(ago),
        "last_seen_at": seen.map(|t| t.to_rfc3339()),
        "online": online,
        "posts": p.post_count,
        "likes": p.likes_received,
        "solved": p.solved_count,
        "reputation": p.reputation,
        "followers": followers,
        "badges": badges,
        "own": own,
        "following": following,
        "can_message": v.user.is_some() && !own,
    });
    // What plugins draw in the card (the `user_card` slot): sanitized HTML.
    let slot_html = crate::platform::plugins::user_card_html(&state, &v, p.id.get()).await;
    if !slot_html.is_empty() {
        card["slot_html"] = json!(slot_html);
    }
    crate::platform::plugins::add_plugin_fields(&state, "user", p.id.get(), &mut card).await;
    let mut res = Json(card).into_response();
    // Who is asking matters (follow state, last seen for staff): never shared between viewers.
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=30"));
    Ok(res)
}

/// The viewer's follow level for a topic or forum, as a template key.
pub(crate) async fn follow_key(state: &AppState, v: &Viewer, target: &str, id: i64) -> &'static str {
    let (Some(u), Some(pool)) = (&v.user, &state.pool) else { return "normal" };
    bf_db::community::follows::follow_level(pool, u.id.get(), target, id).await.map_or("normal", |l| l.key())
}

/// Now a tab of the profile; old links keep working.
async fn user_summary(Path(id): Path<i64>) -> Response {
    Redirect::permanent(&format!("/profile/{id}-x/?tab=summary")).into_response()
}

#[derive(Deserialize)]
struct TitleForm {
    csrf: String,
    title: String,
}

/// Moderators, or the topic's starter while it is open.
async fn change_title(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<TitleForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) && (t.author_id != Some(user.id.get()) || t.locked || v.actor.silenced) {
        return Err(AppError::Forbidden);
    }
    let checked;
    let mut held = false;
    let title = if perms.has(Perms::MODERATE) {
        f.title.trim()
    } else {
        // The same words and account rules as a new topic's title.
        crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
        checked = crate::moderation::gate::words(&state, &v, f.title.trim()).await?;
        held = checked.held;
        checked.text.as_str()
    };
    // The plugins' title filter and every title rule a new topic meets: length, emoji, hidden
    // characters, capitals and, when the site forbids repeats, a title another topic already has.
    let filtered = crate::platform::plugins::filter_text(&state, "item.before_create", "title", title).await?;
    let title = filtered.as_str();
    crate::forum::posting::validate_title(&state, title).map_err(AppError::Validation)?;
    if title != t.title && !state.settings().bool("allow_duplicate_topic_titles") {
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM items WHERE state = 'visible' AND id <> $2 AND lower(title) = lower($1))",
        )
        .bind(title.trim())
        .bind(id)
        .fetch_one(state.db()?)
        .await
        .db()?;
        if taken {
            return Err(AppError::Validation("A topic with this title already exists.".into()));
        }
    }
    if title != t.title && held {
        // That forum's moderators decide, as they do for its held posts.
        let payload = serde_json::json!({ "item": id, "title": title });
        crate::moderation::held::hold(&state, user.id.get(), "title", Some(t.node_id), payload).await?;
        return Ok(crate::moderation::held::held_page(&state, &v));
    }
    if title != t.title {
        bf_db::moderation::topics::retitle(state.db()?, id, title, user.id.get()).await.db()?;
        state.hub.publish(&format!("item:{id}"), "state", serde_json::json!({ "action": "retitled" }));
        state.forum.invalidate();
        state.microcache.clear();
    }
    Ok(Redirect::to(&topic_path(id, &t.slug)).into_response())
}

#[derive(Deserialize)]
struct VisitorsForm {
    csrf: String,
    #[serde(default)]
    on: String,
}

async fn set_visitors(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<VisitorsForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    visitors::set_show_visitors(state.db()?, (v.require_user()?.id).get(), f.on == "1").await.db()?;
    Ok(Redirect::to("/settings").into_response())
}
