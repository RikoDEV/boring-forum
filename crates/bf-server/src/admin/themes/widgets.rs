//! Widgets (the page builder): kinds, the pages that have areas, the admin endpoints and rendering with its cache.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::i18n::en;
use bf_core::perms::Perms;
use bf_db::admin::themes;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

pub(super) const KINDS: [&str; 16] = [
    "forum_list",
    "home_latest",
    "home_stats",
    "online_users",
    "latest_topics",
    "latest_statuses",
    "top_posters",
    "stats",
    "custom",
    "tag_cloud",
    "upcoming_events",
    "calendar",
    "plugin",
    "browsing",
    "suggested_topics",
    "html",
];

/// Blocks the page draws itself with data of its own (who is on it, the topics to read next): not
/// rendered or cached here. `browsing` is drawn by the home, forum and topic pages, one per page;
/// `suggested_topics` by the topic page.
pub(super) const DRAWN_BY_PAGE: [&str; 2] = ["browsing", "suggested_topics"];

/// Where a page-drawn block may go: the pages whose handlers supply its data.
fn page_draws(kind: &str, owner: &str) -> bool {
    match kind {
        "browsing" => matches!(owner, "home" | "forum" | "topic"),
        "suggested_topics" => owner == "topic",
        _ => true,
    }
}

/// Pages with a layout of their own (the page builder): (name, label, icon, group, address to
/// edit it at, empty when there is no single page). Each has the areas `{name}_top`, `_side` and
/// `_bottom`; the home page has `home_top`, `home_main` and `home_bottom`. The admin Widgets page
/// lists them from here. A page opts in with `page_area` (pages built on `layout.html`) or
/// `Shell::with_area` (pages in the shared shell). A new page also needs a migration that adds it
/// to the database's `widgets_area_check` (copy the newest migration that redefines it).
pub(crate) const PAGES: [(&str, &str, &str, &str, &str); 14] = [
    ("home", en("Home page"), "house", en("Forum"), "/"),
    ("forum", en("Forum pages"), "folder", en("Forum"), ""),
    ("topic", en("Topic pages"), "message-square", en("Forum"), ""),
    // /latest and /unread share one layout.
    ("latest", en("Latest and unread"), "history", en("Forum"), "/latest"),
    ("top", en("Top topics"), "trophy", en("Forum"), "/top"),
    ("search", en("Search"), "search", en("Forum"), "/search"),
    ("tag", en("Tag pages"), "tag", en("Forum"), ""),
    ("members", en("Members"), "users", en("Community"), "/u"),
    ("groups", en("Groups"), "shield", en("Community"), "/g"),
    ("badges", en("Badges"), "award", en("Community"), "/badges"),
    ("leaderboard", en("Leaderboard"), "medal", en("Community"), "/leaderboard"),
    ("about", en("About"), "info", en("Community"), "/about"),
    ("clubs", en("Clubs"), "tent", en("Community"), "/clubs"),
    ("live", en("Live Q&A"), "mic", en("Community"), "/live"),
];

/// A known area, or one of a custom page's own (`page12_top`).
pub(super) fn area_ok(area: &str) -> bool {
    let Some((owner, place)) = area.split_once('_') else { return false };
    if owner == "home" {
        return matches!(place, "top" | "main" | "bottom");
    }
    if PAGES.iter().any(|p| p.0 == owner) {
        return matches!(place, "top" | "side" | "bottom");
    }
    let id = owner.strip_prefix("page").unwrap_or_default();
    matches!(place, "top" | "main" | "side" | "bottom" | "col1" | "col2" | "col3")
        && !id.is_empty()
        && id.len() <= 18
        && id.bytes().all(|b| b.is_ascii_digit())
}

/// A custom page only has the areas of its layout; other pages' areas need no check here.
async fn on_layout(state: &AppState, area: &str) -> Result<(), AppError> {
    let Some((owner, place)) = area.split_once('_') else { return Ok(()) };
    let Some(id) = owner.strip_prefix("page").and_then(|n| n.parse::<i64>().ok()) else { return Ok(()) };
    let template = bf_db::admin::pages::template_of(state.db()?, id).await.db()?;
    match template {
        Some(t) if bf_db::admin::pages::places(&t).contains(&place) => Ok(()),
        Some(_) => Err(AppError::Validation("That area is not part of this page's layout.".into())),
        None => Err(AppError::NotFound),
    }
}

/// Drawn by the home page for each viewer, so not rendered or cached here; one of each at most.
pub(super) const PER_VIEWER: [&str; 3] = ["forum_list", "home_latest", "home_stats"];

pub(super) async fn list_widgets(State(state): State<AppState>, Admin(_, v): AdminRead) -> Result<Response, AppError> {
    let pages: Vec<_> = PAGES
        .iter()
        .map(|(name, label, icon, group, url)| {
            json!({ "name": name, "label": v.t().msg(label), "icon": icon, "group": v.t().msg(group), "url": url })
        })
        .collect();
    Ok(Json(json!({ "widgets": themes::widgets(state.db()?).await.db()?, "kinds": KINDS, "pages": pages }))
        .into_response())
}

#[derive(Deserialize)]
pub(super) struct WidgetBody {
    #[serde(default)]
    pub(super) area: String,
    #[serde(default)]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) title: String,
    #[serde(default)]
    pub(super) config: Value,
    #[serde(default = "default_ttl")]
    pub(super) cache_seconds: i32,
}

pub(super) fn default_ttl() -> i32 {
    60
}

/// One line of the staff log for a change to the page builder.
async fn audit(state: &AppState, who: Option<i64>, action: &str, id: i64, details: Value) -> Result<(), AppError> {
    crate::admin::api::audit_log(state.db()?, who, action, &format!("widget:{id}"), details).await
}

pub(super) async fn add_widget(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<WidgetBody>,
) -> Result<Response, AppError> {
    if !KINDS.contains(&b.kind.as_str()) || !area_ok(&b.area) {
        return Err(AppError::invalid(
            "Kind must be one of {n}; area is a page's top, side or bottom (pages: {m}).",
            &[("n", &KINDS.join(", ")), ("m", &PAGES.iter().map(|p| p.0).collect::<Vec<_>>().join(", "))],
        ));
    }
    on_layout(&state, &b.area).await?;
    if PER_VIEWER.contains(&b.kind.as_str()) && !b.area.starts_with("home_") {
        return Err(AppError::Validation("That block only works on the home page.".into()));
    }
    if PER_VIEWER.contains(&b.kind.as_str())
        && themes::widgets(state.db()?).await.db()?.iter().any(|w| w.kind == b.kind)
    {
        return Err(AppError::Validation("That block is already on the page.".into()));
    }
    if DRAWN_BY_PAGE.contains(&b.kind.as_str()) {
        let owner = b.area.split_once('_').map_or("", |(o, _)| o);
        if !page_draws(&b.kind, owner) {
            return Err(AppError::Validation("That block does not work on this page.".into()));
        }
        let taken = themes::widgets(state.db()?)
            .await
            .db()?
            .iter()
            .any(|w| w.kind == b.kind && w.area.split_once('_').map_or("", |(o, _)| o) == owner);
        if taken {
            return Err(AppError::Validation("That block is already on the page.".into()));
        }
    }
    let w = themes::Widget {
        id: 0,
        area: b.area,
        kind: b.kind,
        title: b.title,
        config: crate::platform::plugins::check_widget_config(b.config)?,
        position: 0,
        cache_seconds: b.cache_seconds.clamp(0, 86_400),
    };
    let id = themes::add_widget(state.db()?, &w).await.db()?;
    // The block's own content (its HTML or Markdown) is in `config`.
    audit(
        &state,
        who,
        "widget.add",
        id,
        json!({ "area": w.area, "kind": w.kind, "title": w.title, "config": w.config }),
    )
    .await?;
    changed(&state);
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

/// The page builder changed: the rendered blocks, the lists of them and the page cache go.
fn changed(state: &AppState) {
    state.widgets.invalidate_all();
    state.widget_list.invalidate_all();
    state.microcache.clear();
}

pub(super) async fn update_widget(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<WidgetBody>,
) -> Result<Response, AppError> {
    let w = themes::Widget {
        id,
        area: String::new(),
        kind: String::new(),
        title: b.title,
        config: crate::platform::plugins::check_widget_config(b.config)?,
        position: 0,
        cache_seconds: b.cache_seconds.clamp(0, 86_400),
    };
    themes::update_widget(state.db()?, &w).await.found()?;
    audit(&state, who, "widget.update", id, json!({ "title": w.title, "config": w.config })).await?;
    changed(&state);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn delete_widget(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    themes::delete_widget(state.db()?, id).await.found()?;
    audit(&state, who, "widget.delete", id, json!({})).await?;
    changed(&state);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub(super) struct Arrange {
    pub(super) area: String,
    pub(super) ids: Vec<i64>,
}

pub(super) async fn arrange(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<Arrange>,
) -> Result<Response, AppError> {
    if !area_ok(&b.area) {
        return Err(AppError::Validation("Unknown area.".into()));
    }
    on_layout(&state, &b.area).await?;
    let owner = b.area.split_once('_').map_or("", |(o, _)| o);
    let all = themes::widgets(state.db()?).await.db()?;
    let mut moved = all.iter().filter(|w| b.ids.contains(&w.id));
    if moved.clone().any(|w| DRAWN_BY_PAGE.contains(&w.kind.as_str()) && !page_draws(&w.kind, owner)) {
        return Err(AppError::Validation("That block does not work on this page.".into()));
    }
    if !b.area.starts_with("home_") && moved.any(|w| PER_VIEWER.contains(&w.kind.as_str())) {
        return Err(AppError::Validation("That block only works on the home page.".into()));
    }
    themes::arrange_widgets(state.db()?, &b.area, &b.ids).await.db()?;
    let details = json!({ "ids": b.ids });
    crate::admin::api::audit_log(state.db()?, who, "widget.arrange", &format!("area:{}", b.area), details).await?;
    state.widget_list.invalidate_all();
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Renders one widget. Lists respect what guests may read: the output is cached and shared,
/// so it must never depend on the viewer.
pub(super) async fn render_widget(state: &AppState, w: &themes::Widget) -> Result<String, AppError> {
    let pool = state.db()?;
    let limit = w.config.get("limit").and_then(Value::as_i64).unwrap_or(5).clamp(1, 50);
    let tree = state.forum.get(pool).await?;
    let guest = bf_core::perms::Actor { groups: vec![1.into()], ..Default::default() };
    let public: Vec<i64> =
        tree.nodes.iter().filter(|n| tree.perms(&guest, n.id).has(Perms::READ)).map(|n| n.id).collect();
    Ok(match w.kind.as_str() {
        "latest_statuses" => {
            let rows = bf_db::community::statuses::latest_public(pool, limit).await.db()?;
            if rows.is_empty() {
                return Ok(format!(
                    "<p class=\"muted small\">{}</p>",
                    crate::http::i18n::trh("No status updates yet.")
                ));
            }
            let items: String = rows
                .iter()
                .map(|(name, body, at)| {
                    format!(
                        "<li><img class=\"avatar\" src=\"/avatar/{0}.svg\" width=\"28\" height=\"28\" alt=\"\" loading=\"lazy\"><div><a href=\"/u/{0}\"><strong>{0}</strong></a> <span class=\"muted small\">{2}</span><p>{1}</p></div></li>",
                        esc(name),
                        esc(body),
                        crate::http::render::ago(*at)
                    )
                })
                .collect();
            format!("<ul class=\"bf-status-widget\">{items}</ul>")
        }
        "online_users" => {
            let names = themes::online_users(pool, 15, limit).await.db()?;
            let links: Vec<String> = names.iter().map(|n| format!("<a href=\"/u/{0}\">{0}</a>", esc(n))).collect();
            if links.is_empty() {
                format!("<p class=\"muted small\">{}</p>", crate::http::i18n::trh("Nobody right now."))
            } else {
                format!("<p class=\"small\">{}</p>", links.join(", "))
            }
        }
        "latest_topics" => {
            let rows = themes::latest_titles(pool, &public, limit).await.db()?;
            let items: String = rows
                .iter()
                .map(|(id, t, s)| {
                    format!("<li><a href=\"{}\">{}</a></li>", esc(&bf_core::slug::topic_path(*id, s)), esc(t))
                })
                .collect();
            format!("<ul class=\"small\">{items}</ul>")
        }
        "top_posters" => {
            let rows = themes::top_posters(pool, limit).await.db()?;
            let items: String = rows
                .iter()
                .map(|(n, c)| format!("<li><a href=\"/u/{0}\">{0}</a> <span class=\"muted\">{c}</span></li>", esc(n)))
                .collect();
            format!("<ol class=\"small\">{items}</ol>")
        }
        "stats" => {
            let s = bf_db::admin::maintenance::index_stats(pool).await.db()?;
            format!("<p class=\"small\">{} topics · {} posts · {} members</p>", s.topics, s.posts, s.members)
        }
        // Your own HTML, through the same allow-list as posts: markup and links, no scripts or styles.
        "html" => bf_cook::sanitize(w.config.get("html").and_then(Value::as_str).unwrap_or_default()),
        // Through the cooker, so custom content is as safe as a post.
        "custom" => bf_cook::cook(w.config.get("markdown").and_then(Value::as_str).unwrap_or_default()).html,
        "plugin" => {
            let text = |k: &str| w.config.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            crate::platform::plugins::render_widget(
                state,
                &text("plugin"),
                &text("key"),
                w.config.get("settings").unwrap_or(&Value::Null),
            )
            .await
        }
        "tag_cloud" => {
            let rows = themes::tag_cloud(pool, &public, limit.max(20)).await.db()?;
            rows.iter().map(|(n, s, _)| format!("<a class=\"tag\" href=\"/tag/{}\">{}</a> ", esc(s), esc(n))).collect()
        }
        // config.events: [{"date": "2027-01-31", "title": "...", "url": "/..."}]; past ones drop off.
        "upcoming_events" if w.config.get("events").and_then(Value::as_array).is_none_or(Vec::is_empty) => {
            // Nothing listed by hand: the next events of the calendar.
            let today = chrono::Utc::now().date_naive();
            let rows =
                bf_db::community::events::events_between(pool, &public, today, today + chrono::Duration::days(90))
                    .await
                    .db()?;
            let items: String = rows
                .iter()
                .filter(|(e, _, _)| e.starts_at >= chrono::Utc::now() - chrono::Duration::hours(3))
                .take(limit as usize)
                .map(|(e, slug, title)| {
                    format!(
                        "<li>{}: <a href=\"{}\">{}</a></li>",
                        bf_web::ago::Ago::day(e.starts_at),
                        esc(&bf_core::slug::topic_path(e.item_id, slug)),
                        esc(title)
                    )
                })
                .collect();
            if items.is_empty() {
                format!("<p class=\"muted small\">{}</p>", crate::http::i18n::trh("Nothing coming up."))
            } else {
                format!("<ul class=\"small\">{items}</ul>")
            }
        }
        // A small month grid of the calendar's events in forums guests may read; a day with events
        // links to its topic, or to the month when it has several.
        "calendar" => {
            use chrono::Datelike;
            let today = chrono::Utc::now().date_naive();
            let first = today.with_day(1).unwrap_or(today);
            let next = if first.month() == 12 {
                chrono::NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
            } else {
                chrono::NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
            }
            .unwrap_or(first);
            let last = next - chrono::Duration::days(1);
            let start = first - chrono::Duration::days(i64::from(first.weekday().num_days_from_monday()));
            let end = last + chrono::Duration::days(i64::from(6 - last.weekday().num_days_from_monday()));
            let rows = bf_db::community::events::events_between(pool, &public, start, end).await.db()?;
            let month = first.format("%Y-%m");
            let mut html = format!(
                "<div class=\"bf-minical\"><a class=\"bf-minical-title\" href=\"/calendar?month={month}\">{}</a><table><thead><tr>",
                bf_web::ago::Ago::month_long(first.and_hms_opt(12, 0, 0).unwrap_or_default().and_utc())
            );
            for d in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
                html.push_str(&format!("<th scope=\"col\">{}</th>", crate::http::i18n::trh(d)));
            }
            html.push_str("</tr></thead><tbody>");
            let mut day = start;
            while day <= end {
                html.push_str("<tr>");
                for _ in 0..7 {
                    let key = day.format("%Y-%m-%d").to_string();
                    let here: Vec<_> = rows.iter().filter(|(e, _, _)| e.local_start.starts_with(&key)).collect();
                    let class = [
                        (day.month() != first.month()).then_some("is-other"),
                        (day == today).then_some("is-today"),
                        (!here.is_empty()).then_some("has-events"),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                    let cell = match here.as_slice() {
                        [] => format!("{}", day.day()),
                        [(e, slug, title)] => format!(
                            "<a href=\"{}\" data-tip=\"{}\">{}</a>",
                            esc(&bf_core::slug::topic_path(e.item_id, slug)),
                            esc(title),
                            day.day()
                        ),
                        many => format!(
                            "<a href=\"/calendar?month={}\" data-tip=\"{}\">{}</a>",
                            day.format("%Y-%m"),
                            esc(&many.iter().map(|(_, _, t)| t.as_str()).collect::<Vec<_>>().join(" · ")),
                            day.day()
                        ),
                    };
                    html.push_str(&format!("<td class=\"{class}\">{cell}</td>"));
                    day += chrono::Duration::days(1);
                }
                html.push_str("</tr>");
            }
            html.push_str("</tbody></table>");
            let soon: Vec<_> = rows
                .iter()
                .filter(|(e, _, _)| e.starts_at >= chrono::Utc::now() - chrono::Duration::hours(3))
                .take(limit as usize)
                .collect();
            if soon.is_empty() {
                html.push_str(&format!(
                    "<p class=\"muted small\">{}</p>",
                    crate::http::i18n::trh("Nothing coming up.")
                ));
            } else {
                html.push_str("<ul class=\"small\">");
                for (e, slug, title) in soon {
                    html.push_str(&format!(
                        "<li>{}: <a href=\"{}\">{}</a></li>",
                        bf_web::ago::Ago::day(e.starts_at),
                        esc(&bf_core::slug::topic_path(e.item_id, slug)),
                        esc(title)
                    ));
                }
                html.push_str("</ul>");
            }
            html.push_str("</div>");
            html
        }
        "upcoming_events" => {
            let today = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string();
            let mut rows: Vec<&Value> = w
                .config
                .get("events")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|e| e["date"].as_str().is_some_and(|d| d >= today.as_str()))
                .collect();
            rows.sort_by_key(|e| e["date"].as_str().unwrap_or_default().to_owned());
            let items: String = rows
                .iter()
                .take(limit as usize)
                .map(|e| {
                    let (date, title) =
                        (esc(e["date"].as_str().unwrap_or_default()), esc(e["title"].as_str().unwrap_or_default()));
                    match e["url"]
                        .as_str()
                        .filter(|u| u.starts_with('/') && !u.starts_with("//") || u.starts_with("https://"))
                    {
                        Some(u) => format!("<li>{date}: <a href=\"{}\">{title}</a></li>", esc(u)),
                        None => format!("<li>{date}: {title}</li>"),
                    }
                })
                .collect();
            if items.is_empty() {
                format!("<p class=\"muted small\">{}</p>", crate::http::i18n::trh("Nothing coming up."))
            } else {
                format!("<ul class=\"small\">{items}</ul>")
            }
        }
        _ => String::new(),
    })
}

/// The home page's widgets as (top, bottom), each (title, html); cached per widget's own TTL.
pub(crate) async fn home_widgets(
    state: &AppState,
) -> (Vec<(i64, String, String, String)>, Vec<(i64, String, String, String)>, Vec<(i64, String, String, String)>) {
    let Some(pool) = &state.pool else { return Default::default() };
    let all = if let Some(w) = state.widget_list.get(&()) {
        w
    } else {
        // A failed read must not leave a page with no forums on it; it is not cached.
        let Ok(rows) = themes::widgets(pool).await else {
            let block = |k: &str| (0, String::new(), String::new(), k.to_owned());
            return (vec![], vec![block("forum_list")], vec![block("home_latest"), block("home_stats")]);
        };
        let w = std::sync::Arc::new(rows);
        state.widget_list.insert((), w.clone());
        w
    };
    let (mut top, mut main, mut bottom) = (Vec::new(), Vec::new(), Vec::new());
    for w in all.iter() {
        let list = match w.area.as_str() {
            "home_top" => &mut top,
            "home_main" => &mut main,
            "home_bottom" => &mut bottom,
            _ => continue,
        };
        if PER_VIEWER.contains(&w.kind.as_str()) {
            list.push((w.id, String::new(), String::new(), w.kind.clone()));
        } else if let Some(entry) = rendered(state, w).await {
            list.push(entry);
        }
    }
    (top, main, bottom)
}

/// One widget's cached output: (id, title, html, kind). None if it failed, which must not take
/// the page with it.
pub(super) async fn rendered(state: &AppState, w: &themes::Widget) -> Option<(i64, String, String, String)> {
    // The page fills these in for each viewer.
    if DRAWN_BY_PAGE.contains(&w.kind.as_str()) {
        return Some((w.id, crate::http::i18n::lang().msg(&w.title), String::new(), w.kind.clone()));
    }
    let key = (w.id, crate::http::i18n::lang().locale);
    let fresh =
        state.widgets.get(&key).filter(|(at, _)| at.elapsed() < Duration::from_secs((w.cache_seconds as u64).min(300)));
    let html = match fresh {
        Some((_, html)) => html,
        None => match render_widget(state, w).await {
            Ok(html) => {
                state.widgets.insert(key, (std::time::Instant::now(), html.clone()));
                html
            }
            Err(e) => {
                tracing::warn!(widget = w.id, error = %e, "widget failed");
                return None;
            }
        },
    };
    Some((w.id, crate::http::i18n::lang().msg(&w.title), html, w.kind.clone()))
}

/// A page's widget areas, for a page built on `layout.html` (which draws them around the
/// content); `editing` opens the layout editor, for admins only.
pub(crate) async fn page_area(
    state: &AppState,
    v: &Viewer,
    page: &str,
    editing: bool,
) -> Option<bf_web::pages::PageArea> {
    Some(bf_web::pages::PageArea {
        name: page.to_owned(),
        widgets: page_widgets(state, page).await,
        editing: editing && v.actor.is_admin,
    })
}

/// The blocks around a page: `page` is one of `PAGES` (or a custom page's own). These blocks are
/// global and guest-safe, so a private forum's page shows nothing it should not.
pub(crate) async fn page_widgets(state: &AppState, page: &str) -> bf_web::pages::PageWidgets {
    let mut out = bf_web::pages::PageWidgets::default();
    let Some(pool) = &state.pool else { return out };
    let all = if let Some(w) = state.widget_list.get(&()) {
        w
    } else {
        let Ok(rows) = themes::widgets(pool).await else { return out };
        let w = std::sync::Arc::new(rows);
        state.widget_list.insert((), w.clone());
        w
    };
    for w in all.iter() {
        let Some(place) = w.area.strip_prefix(page).and_then(|a| a.strip_prefix('_')) else { continue };
        let list = match place {
            "top" => &mut out.top,
            "side" => &mut out.side,
            "bottom" => &mut out.bottom,
            "main" => &mut out.main,
            "col1" => &mut out.col1,
            "col2" => &mut out.col2,
            "col3" => &mut out.col3,
            _ => continue,
        };
        if let Some(entry) = rendered(state, w).await {
            list.push(entry);
        }
    }
    out
}
