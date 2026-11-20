//! Pages about the community: members, groups, badges, the leaderboard and /about.
//! Promotion rules, warnings, announcements and member directories.

use crate::http::render::{DbResult, layout, page};
use crate::http::ui::simple_page;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use bf_core::AppError;
use bf_core::html::esc;
use bf_db::community::{directory, members};
use bf_web::pages::{AboutPage, BadgePage, BadgesPage, GroupsPage, LeaderboardPage, UsersPage, render};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/u", get(users))
        .route("/g", get(groups))
        .route("/badges", get(badges))
        .route("/badges/{id}", get(badge))
        .route("/leaderboard", get(leaderboard))
        .route("/about", get(about))
        .route("/staff", get(staff))
        .route("/online", get(online))
}

const USERS_PER_PAGE: i64 = 50;

#[derive(Deserialize)]
struct UsersQuery {
    #[serde(default)]
    sort: String,
    #[serde(default)]
    q: String,
    page: Option<i64>,
}

async fn users(
    State(state): State<AppState>,
    v: Viewer,
    Query(u): Query<UsersQuery>,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    let sort = match u.sort.as_str() {
        s @ ("reputation" | "joined" | "name") => s,
        _ => "posts",
    };
    let q = u.q.trim().to_owned();
    let q =
        if q.is_empty() { q } else { crate::platform::plugins::filter_text(&state, "search.query", "q", &q).await? };
    let n = u.page.unwrap_or(1).clamp(1, 10_000);
    let mut rows =
        directory::members(state.db()?, sort, &q, USERS_PER_PAGE + 1, (n - 1) * USERS_PER_PAGE).await.db()?;
    let more = rows.len() as i64 > USERS_PER_PAGE;
    rows.truncate(USERS_PER_PAGE as usize);
    let url = |sort: &str, n: i64| {
        let mut s = url::form_urlencoded::Serializer::new(String::new());
        if sort != "posts" {
            s.append_pair("sort", sort);
        }
        if !q.is_empty() {
            s.append_pair("q", &q);
        }
        if n > 1 {
            s.append_pair("page", &n.to_string());
        }
        let s = s.finish();
        if s.is_empty() { "/u".to_owned() } else { format!("/u?{s}") }
    };
    let show_levels = state.settings().bool("show_trust_levels");
    let (mut l, set) = layout(&state, &v, Some(&v.t().t("footer.members")));
    l.noindex = !q.is_empty() || n > 1 || sort != "posts";
    l.area = crate::admin::themes::page_area(&state, &v, "members", edit).await;
    let (prev_url, next_url) = ((n > 1).then(|| url(sort, n - 1)), more.then(|| url(sort, n + 1)));
    l.set_paging(&state.config.base_url, prev_url.as_deref(), next_url.as_deref());
    let p = UsersPage {
        layout: l,
        q: q.clone(),
        sort: sort.to_owned(),
        sorts: [
            ("posts", "users.sort_posts"),
            ("reputation", "post.reputation"),
            ("joined", "users.sort_newest"),
            ("name", "users.sort_name"),
        ]
        .into_iter()
        .map(|(k, key)| (v.t().t(key), url(k, 1), k == sort))
        .collect(),
        rows: rows
            .into_iter()
            .map(|m| {
                let level = show_levels
                    .then(|| (m.trust_level, crate::community::trust::level_label(&state, &v.t(), m.trust_level)));
                (m.username, m.group_name, m.post_count, m.reputation, bf_web::ago::Ago::month(m.created_at), level)
            })
            .collect(),
        prev_url,
        next_url,
        page: n,
    };
    Ok(page(render(&p), set, v.user.is_none()))
}

async fn groups(
    State(state): State<AppState>,
    v: Viewer,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    let groups = directory::groups(state.db()?).await.db()?;
    let (mut l, set) = layout(&state, &v, Some(&v.t().t("footer.groups")));
    l.area = crate::admin::themes::page_area(&state, &v, "groups", edit).await;
    Ok(page(render(&GroupsPage { layout: l, groups }), set, v.user.is_none()))
}

async fn badges(
    State(state): State<AppState>,
    v: Viewer,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    let badges = directory::badges(state.db()?).await.db()?;
    let (mut l, set) = layout(&state, &v, Some(&v.t().t("footer.badges")));
    l.area = crate::admin::themes::page_area(&state, &v, "badges", edit).await;
    let t = v.t();
    let badges = badges
        .into_iter()
        .map(|b| {
            (
                b.id,
                t.msg(&b.name),
                t.msg(&b.description),
                b.points,
                b.holders,
                b.tier,
                b.icon,
                t.msg(&b.category),
                b.color,
                b.level,
            )
        })
        .collect::<Vec<_>>();
    Ok(page(
        render(&BadgesPage {
            layout: l,
            groups: badges.into_iter().fold(
                Vec::new(),
                |mut groups: Vec<(String, Vec<_>)>,
                 (id, name, about, points, holders, tier, icon, category, color, level)| {
                    let row = (id, name, about, points, holders, tier, icon, color, level);
                    match groups.last_mut() {
                        Some((g, rows)) if *g == category => rows.push(row),
                        _ => groups.push((category, vec![row])),
                    }
                    groups
                },
            ),
        }),
        set,
        v.user.is_none(),
    ))
}

async fn badge(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<String>,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    let id: i64 = id.parse().map_err(|_| AppError::NotFound)?;
    let pool = state.db()?;
    let b = directory::badge(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let holders = directory::badge_holders(pool, id, 200).await.db()?;
    let t = v.t();
    let (name, description) = (t.msg(&b.name), t.msg(&b.description));
    let (mut l, set) = layout(&state, &v, Some(&name));
    l.area = crate::admin::themes::page_area(&state, &v, "badges", edit).await;
    let p = BadgePage {
        tier: b.tier,
        icon: b.icon,
        color: b.color,
        level: b.level,
        layout: l,
        name,
        description,
        points: b.points,
        total: b.holders,
        holders: holders.into_iter().map(|(n, at)| (n, bf_web::ago::Ago::day(at))).collect(),
    };
    Ok(page(render(&p), set, v.user.is_none()))
}

async fn leaderboard(
    State(state): State<AppState>,
    v: Viewer,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    let rows = directory::leaderboard(state.db()?, 50).await.db()?;
    let (mut l, set) = layout(&state, &v, Some(&v.t().t("footer.leaderboard")));
    l.area = crate::admin::themes::page_area(&state, &v, "leaderboard", edit).await;
    let rows = rows.into_iter().enumerate().map(|(i, (n, rep, posts))| (i + 1, n, rep, posts)).collect();
    Ok(page(render(&LeaderboardPage { layout: l, rows }), set, v.user.is_none()))
}

async fn about(
    State(state): State<AppState>,
    v: Viewer,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let stats = directory::about_stats(pool).await.db()?;
    let staff = bf_db::community::members::staff(pool).await.db()?;
    let settings = state.settings();
    let (mut l, set) = layout(&state, &v, Some(&v.t().t("footer.about")));
    l.area = crate::admin::themes::page_area(&state, &v, "about", edit).await;
    let p = AboutPage {
        layout: l,
        contact: !settings.str("contact_email").is_empty(),
        members: stats.members,
        topics: stats.topics,
        posts: stats.posts,
        online: stats.online,
        since: stats.since.map(bf_web::ago::Ago::month_long).unwrap_or_default(),
        staff,
    };
    Ok(page(render(&p), set, v.user.is_none()))
}

fn member_link(name: &str) -> String {
    format!("<a href=\"/u/{0}\">{0}</a>", esc(name))
}

async fn staff(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let rows = members::staff(state.db()?).await.db()?;
    let items: String = rows
        .iter()
        .map(|(n, g)| format!("<li>{} <span class=\"muted small\">{}</span></li>", member_link(n), esc(g)))
        .collect();
    Ok(simple_page(&state, &v, &v.t().t("about.staff_title"), &format!("<ul class=\"activity\">{items}</ul>"), true))
}

async fn online(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let names = bf_db::admin::themes::online_users(state.db()?, 15, 200).await.db()?;
    let html = if names.is_empty() {
        format!("<p class=\"muted\">{}</p>", esc(&v.t().t("about.nobody_online")))
    } else {
        let links: Vec<String> = names.iter().map(|n| member_link(n)).collect();
        format!("<p>{}</p>", links.join(", "))
    };
    Ok(simple_page(&state, &v, &v.t().t("about.online"), &html, false))
}
