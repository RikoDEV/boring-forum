//! Moderator panel and admin tools.

use crate::admin::api::admin;
use crate::http::render::{DbResult, layout, page};
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::moderation::tools;
use bf_web::pages::{ModBan, ModCpPage, ModLogRow, ModMember, ModWarning, render};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/ip/{ip}", get(by_ip))
        .route("/api/v1/admin/users/{id}/history", get(history))
        .route("/modcp", get(modcp))
        .route("/api/v1/admin/users/{id}/reset-2fa", post(reset_2fa))
        .route("/api/v1/admin/users/{id}/ips", get(user_ips))
        .route("/api/v1/admin/users/{id}", delete(delete_user))
}

#[derive(Deserialize)]
struct ModCpQuery {
    #[serde(default)]
    tab: String,
    #[serde(default)]
    ip: String,
    #[serde(default)]
    find: String,
    #[serde(default)]
    page: i64,
    /// What the last action did, to say so once.
    #[serde(default)]
    done: String,
}

const LOG_PAGE: i64 = 40;

/// What a moderator did, in words.
fn describe(t: &bf_core::i18n::T, action: &str) -> String {
    match action {
        "post.hide" => t.msg("hid a post"),
        "post.unhide" => t.msg("showed a hidden post again"),
        "post.restore" => t.msg("restored a deleted post"),
        "post.delete" => t.msg("deleted a post"),
        "review.approved" => t.msg("kept a reviewed post"),
        "review.rejected" => t.msg("removed a reviewed post"),
        "review.ignored" => t.msg("dismissed a report on"),
        "held.approve" => t.msg("approved held content"),
        "held.reject" => t.msg("rejected held content"),
        "topic.action" => t.msg("ran a moderator action on"),
        "topic.locked" => t.msg("locked"),
        "topic.unlocked" => t.msg("unlocked"),
        "topic.merge" => t.msg("merged"),
        "topic.move" => t.msg("moved"),
        "topic.assign" => t.msg("assigned"),
        "topic.copy" => t.msg("copied"),
        "topic.change_author" => t.msg("changed the author of"),
        "topic.date" => t.msg("changed the date of"),
        "post.edit_lock" => t.msg("locked the editing of a post"),
        "post.edit_unlock" => t.msg("allowed editing a post again"),
        "post.anonymous" => t.msg("posted anonymously in"),
        "revision.hide" => t.msg("hid an earlier version of a post"),
        "revision.show" => t.msg("showed an earlier version of a post again"),
        "revision.delete" => t.msg("deleted an earlier version of a post"),
        "upload.known_image" => t.msg("suspended an account after an upload matched the known-image list:"),
        "user.reset_2fa" => t.msg("reset the two-factor sign-in of"),
        "user.delete" => t.msg("deleted the account of"),
        "ban.remove" => t.msg("removed a ban:"),
        "mod_action.create" => t.msg("created a saved moderator action:"),
        "mod_action.update" => t.msg("changed a saved moderator action:"),
        "mod_action.delete" => t.msg("deleted a saved moderator action:"),
        "report_reason.create" => t.msg("created a report reason:"),
        "report_reason.update" => t.msg("changed a report reason:"),
        "report_reason.delete" => t.msg("deleted a report reason:"),
        "dsa.appeal" => t.msg("decided an appeal against"),
        "ban.add" => t.msg("added a ban:"),
        "mod.silence" => t.msg("silenced"),
        "mod.suspend" => t.msg("suspended"),
        "mod.lift" => t.msg("lifted the restrictions of"),
        "mod.unban" => t.msg("lifted a ban:"),
        "mod.spammer" | "user.delete_spammer" => t.msg("cleaned up after the spammer"),
        "user.warn" => t.msg("warned"),
        "user.approve" => t.msg("approved the account of"),
        "user.reject" => t.msg("rejected the account of"),
        "member_report.close" => t.msg("closed a report on a member:"),
        other => t.msgf("did {action} on", &[("action", other)]),
    }
}

/// The place an action happened, as (link, words).
fn place(t: &bf_core::i18n::T, target: Option<&str>, member: Option<&str>) -> (String, String) {
    if let Some(name) = member {
        return (format!("/u/{}", name.to_lowercase()), name.to_owned());
    }
    let Some((kind, id)) = target.and_then(|x| x.split_once(':')) else {
        return (String::new(), target.unwrap_or_default().chars().take(60).collect());
    };
    match (kind, id.parse::<i64>()) {
        ("post", Ok(n)) => (format!("/p/{n}"), t.msgf("post #{n}", &[("n", &n.to_string())])),
        ("user", Ok(n)) => (String::new(), t.msgf("member #{n}", &[("n", &n.to_string())])),
        ("item", Ok(n)) => (format!("/t/x/{n}"), t.msgf("topic #{n}", &[("n", &n.to_string())])),
        _ => (String::new(), id.chars().take(60).collect()),
    }
}

fn pager(base: &str, page: i64, more: bool) -> (Option<String>, Option<String>) {
    let join = if base.contains('?') { '&' } else { '?' };
    let url = |n: i64| if n <= 1 { base.to_owned() } else { format!("{base}{join}page={n}") };
    ((page > 1).then(|| url(page - 1)), more.then(|| url(page + 1)))
}

async fn modcp(State(state): State<AppState>, v: Viewer, Query(q): Query<ModCpQuery>) -> Result<Response, AppError> {
    v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let nodes: Vec<i64> =
        tree.nodes.iter().filter(|n| tree.perms(&v.actor, n.id).has(Perms::MODERATE)).map(|n| n.id).collect();
    if nodes.is_empty() {
        return Err(AppError::Forbidden);
    }
    let t = v.t();
    let staff = is_staff(&v);
    let tab = match q.tab.as_str() {
        "members" | "log" | "bans" if staff => q.tab.clone(),
        "tools" => "tools".into(),
        _ => "overview".into(),
    };
    let (queue, hidden, deleted) = tools::modcp_counts(pool, &nodes).await.db()?;
    let mut tabs = vec![("overview".to_owned(), t.msg("Overview"), "/modcp".to_owned(), 0)];
    if staff {
        tabs.push(("members".into(), t.msg("Members"), "/modcp?tab=members".into(), 0));
        tabs.push(("bans".into(), t.msg("Bans"), "/modcp?tab=bans".into(), 0));
        tabs.push(("log".into(), t.msg("Log"), "/modcp?tab=log".into(), 0));
    }
    tabs.push(("tools".into(), t.msg("Tools"), "/modcp?tab=tools".into(), 0));
    tabs.push(("queue".into(), t.msg("Review queue"), "/review".into(), queue));

    let page_n = q.page.clamp(1, 1000);
    let (mut l, set) = layout(&state, &v, Some(&t.msg("Moderator panel")));
    l.noindex = true;
    let mut p = ModCpPage {
        layout: l,
        tab: tab.clone(),
        tabs,
        queue,
        hidden,
        deleted,
        unapproved: 0,
        oldest: None,
        staff,
        reports: vec![],
        members: vec![],
        find: q.find.trim().chars().take(40).collect(),
        warnings: vec![],
        log: vec![],
        prev_url: None,
        next_url: None,
        doubted: vec![],
        hidden_posts: vec![],
        ip: String::new(),
        ip_members: vec![],
        notice: match q.done.as_str() {
            "silence" => t.msg("The member is silenced and has been told why."),
            "suspend" => t.msg("The member is suspended and has been told why."),
            "lift" => t.msg("The restrictions are lifted."),
            "spammer" => t.msg("The spammer's posts are deleted and the account is suspended."),
            "warn" => t.msg("The warning is given."),
            "ban" => t.msg("The ban is added."),
            "unban" => t.msg("The ban is lifted."),
            _ => String::new(),
        },
        back: format!(
            "/modcp?tab={tab}&find={}&page={page_n}",
            url::form_urlencoded::byte_serialize(q.find.trim().chars().take(40).collect::<String>().as_bytes())
                .collect::<String>()
        ),
        bans: vec![],
        admin: v.actor.is_admin,
    };
    let me = v.user.as_ref().map_or(0, |u| u.id.get());
    let viewer_admin = v.actor.is_admin;
    let member_rows = |rows: Vec<tools::MemberRow>| -> Vec<ModMember> {
        rows.into_iter()
            .map(|m| {
                let mut flags = vec![];
                if !m.approved {
                    flags.push(t.msg("Awaiting approval"));
                }
                if m.suspended {
                    flags.push(t.msg("Suspended"));
                }
                if m.silenced {
                    flags.push(t.msg("Silenced"));
                }
                let can_act = m.id != me && !m.is_admin && (viewer_admin || !m.is_moderator);
                ModMember {
                    id: m.id,
                    can_act,
                    restricted: m.suspended || m.silenced,
                    new_account: m.trust_level <= 1 && !m.is_moderator,
                    name: m.username,
                    joined: bf_web::ago::Ago::new(m.joined_at),
                    seen: m.last_seen_at.map_or_else(|| bf_web::ago::Ago::plain(t.msg("never")), bf_web::ago::Ago::new),
                    posts: m.post_count,
                    trust: m.trust_level,
                    warning_points: m.warning_points,
                    flags,
                }
            })
            .collect()
    };
    let log_rows = |rows: Vec<tools::ModLogEntry>| -> Vec<ModLogRow> {
        rows.into_iter()
            .map(|a| ModLogRow {
                actor: a.actor.unwrap_or_else(|| "deleted".into()),
                what: describe(&t, &a.action),
                note: a.note.unwrap_or_default(),
                target: place(&t, a.target.as_deref(), a.target_name.as_deref()),
                when: bf_web::ago::Ago::new(a.created_at),
            })
            .collect()
    };
    let warning_rows = |rows: Vec<tools::WarningRow>| -> Vec<ModWarning> {
        rows.into_iter()
            .map(|w| ModWarning {
                member: w.member,
                moderator: w.moderator.unwrap_or_default(),
                points: w.points,
                reason: w.reason,
                when: bf_web::ago::Ago::new(w.created_at),
                expires: bf_web::ago::Ago::day(w.expires_at),
            })
            .collect()
    };

    match tab.as_str() {
        "overview" => {
            p.oldest = tools::oldest_pending(pool, &nodes).await.db()?.map(bf_web::ago::Ago::new);
            p.reports = bf_db::moderation::review::review_queue(pool, &nodes, None, true)
                .await
                .db()?
                .into_iter()
                .take(6)
                .map(|r| {
                    (
                        format!("{}/{}", topic_path(r.item_id, &r.slug), r.post_number),
                        r.title,
                        r.author.unwrap_or_else(|| "deleted".into()),
                        r.reasons.unwrap_or_default().join(", ").replace('_', " "),
                        bf_web::ago::Ago::new(r.created_at),
                    )
                })
                .collect();
            if staff {
                p.unapproved = tools::unapproved_members(pool).await.db()?;
                p.members = member_rows(tools::members(pool, "", 6, 0).await.db()?);
                p.warnings = warning_rows(tools::recent_warnings(pool, 5).await.db()?);
                p.log = log_rows(tools::mod_log(pool, 8, 0).await.db()?);
                let s = state.settings();
                if s.bool("ai_enabled") {
                    let (spam_at, tox_at) = (s.int("ai_spam_threshold") as i16, s.int("ai_toxicity_threshold") as i16);
                    p.doubted = bf_db::ai::flagged_posts(pool, tox_at, spam_at)
                        .await
                        .db()?
                        .into_iter()
                        .take(5)
                        .map(|(post, _, title, author, spam, abuse)| (format!("/p/{post}"), title, author, spam, abuse))
                        .collect();
                }
            }
        }
        "members" => {
            let rows = tools::members(pool, &p.find, 31, (page_n - 1) * 30).await.db()?;
            let more = rows.len() > 30;
            p.members = member_rows(rows.into_iter().take(30).collect());
            let find: String = url::form_urlencoded::byte_serialize(p.find.as_bytes()).collect();
            (p.prev_url, p.next_url) = pager(&format!("/modcp?tab=members&find={find}"), page_n, more);
        }
        "bans" => {
            p.bans = bf_db::moderation::bans::list(pool)
                .await
                .db()?
                .into_iter()
                .map(|b| ModBan {
                    id: b.id,
                    kind: b.kind,
                    value: b.value,
                    reason: b.reason.unwrap_or_default(),
                    when: bf_web::ago::Ago::new(b.created_at),
                    expires: b.expires_at.map(bf_web::ago::Ago::stamp),
                })
                .collect();
        }
        "log" => {
            let rows = tools::mod_log(pool, LOG_PAGE + 1, (page_n - 1) * LOG_PAGE).await.db()?;
            let more = rows.len() as i64 > LOG_PAGE;
            p.log = log_rows(rows.into_iter().take(LOG_PAGE as usize).collect());
            p.warnings = warning_rows(tools::recent_warnings(pool, 20).await.db()?);
            (p.prev_url, p.next_url) = pager("/modcp?tab=log", page_n, more);
        }
        _ => {
            p.hidden_posts = tools::hidden_posts(pool, &nodes)
                .await
                .db()?
                .into_iter()
                .map(|(_, item, slug, title, author, reason, n)| {
                    (
                        format!("{}/{n}", topic_path(item, &slug)),
                        title,
                        author.unwrap_or_else(|| "deleted".into()),
                        reason.unwrap_or_default(),
                    )
                })
                .collect();
            // Who posted from an address is for global staff: a moderator of one forum (or a club's
            // owner) has no business with members' addresses elsewhere on the site.
            let ip = if staff { q.ip.trim().to_owned() } else { String::new() };
            let net = ip.parse::<sqlx::types::ipnetwork::IpNetwork>().ok();
            if !ip.is_empty() && net.is_none() {
                return Err(AppError::Validation("Enter an IP address or network.".into()));
            }
            // A wide network would list members by address range, and narrowing it step by step
            // would find anyone's address.
            if net.is_some_and(|n| n.prefix() < if n.is_ipv4() { 24 } else { 48 }) {
                return Err(AppError::Validation("A network can be at most a /24 (IPv4) or /48 (IPv6).".into()));
            }
            if net.is_some() {
                p.ip_members = tools::members_on_ip(pool, &ip)
                    .await
                    .db()?
                    .into_iter()
                    .map(|m| (format!("/u/{}", m.username), m.username, m.posts, bf_web::ago::Ago::day(m.last_used)))
                    .collect();
            }
            p.ip = ip;
        }
    }
    Ok(page(render(&p), set, false))
}

async fn reset_2fa(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    if !tools::reset_two_factor(state.db()?, id, who.unwrap_or(0)).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn user_ips(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(tools::user_ips(state.db()?, id).await.db()?).into_response())
}

#[derive(Deserialize)]
struct DeleteUserQuery {
    #[serde(default)]
    content: String,
}

async fn delete_user(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<DeleteUserQuery>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    if who == Some(id) {
        return Err(AppError::Validation("You cannot delete your own account here.".into()));
    }
    let done = tools::delete_user(state.db()?, id, q.content == "delete", who.unwrap_or(0)).await.db()?;
    crate::moderation::topics::refresh(&state);
    match done {
        None => Err(AppError::Validation("The last administrator cannot be deleted.".into())),
        Some(false) => Err(AppError::NotFound),
        Some(true) => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn by_ip(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(ip): Path<String>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    if ip.parse::<std::net::IpAddr>().is_err() {
        return Err(AppError::Validation("That is not an IP address.".into()));
    }
    let rows = tools::accounts_on_ip(state.db()?, &ip).await.db()?;
    let list: Vec<_> =
        rows.iter().map(|(id, name, posts)| json!({ "id": id, "username": name, "posts": posts })).collect();
    Ok(Json(list).into_response())
}

async fn history(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(tools::member_history(state.db()?, id).await.db()?).into_response())
}
