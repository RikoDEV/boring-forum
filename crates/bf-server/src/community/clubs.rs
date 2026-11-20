//! Clubs: a member-owned forum plus a group that holds its members. Open clubs can
//! be read by anyone and joined freely; a closed club's forum is visible to its members only.
//! Also the PWA files, and the redirect map used by the 404 handler.

use crate::http::i18n::{lang, tr, trf, trh};
use crate::http::render::DbResult;
use crate::http::ui::hidden_csrf;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::perms::Perms;
use bf_core::slug::{forum_path, slugify};
use bf_db::community::clubs;
use serde::Deserialize;
use serde_json::json;

/// Invitations sent by one member per hour.
static CLUB_INVITES_BY_USER: std::sync::LazyLock<crate::http::ratelimit::KeyedLimiter> =
    std::sync::LazyLock::new(|| crate::http::ratelimit::KeyedLimiter::new(30, std::time::Duration::from_secs(3600)));

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/clubs", get(list).post(create))
        .route("/clubs/{id}/join", post(join))
        .route("/clubs/{id}/leave", post(leave))
        .route("/clubs/{id}/request", post(request))
        .route("/clubs/{id}/withdraw", post(withdraw))
        .route("/clubs/{id}/decline", post(decline))
        .route("/clubs/{id}/describe", post(describe))
        .route("/clubs/{id}/invite", post(invite))
        .route("/clubs/{id}/requests/{user}", post(decide))
        .route("/clubs/{id}/members/{user}/remove", post(remove_member))
        .route("/clubs/{id}/transfer", post(transfer))
        .route("/clubs/{id}/archive", post(archive))
        .route("/clubs/{id}/delete", post(delete))
        .route("/manifest.webmanifest", get(manifest))
        .route("/favicon.ico", get(favicon))
        .route("/sw.js", get(service_worker))
}

#[derive(Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    show: String,
    #[serde(default)]
    q: String,
}

/// A small coloured mark with the club's first letter; the colour follows the name.
fn mark(name: &str) -> String {
    let letter: String =
        name.chars().find(|c| c.is_alphanumeric()).map_or_else(|| "#".into(), |c| c.to_uppercase().collect());
    let hue = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b))) % 8;
    format!("<span class=\"bf-club-mark bf-club-c{hue}\" aria-hidden=\"true\">{}</span>", esc(&letter))
}

/// A member's name as a link to their profile.
fn user_link(id: i64, name: &str) -> String {
    format!("<a href=\"{}\">{}</a>", esc(&bf_core::slug::profile_path(id, name)), esc(name))
}

/// The one thing this viewer can do with a club, as a button (or a note).
fn action(c: &clubs::Club, viewer: Option<i64>, token: &str) -> String {
    let form = |path: &str, label: &str, class: &str, confirm: Option<&str>| {
        let confirm = confirm.map(|m| format!(" data-confirm=\"{}\"", esc(m))).unwrap_or_default();
        format!(
            "<form method=\"post\" action=\"/clubs/{}/{path}\" class=\"inline\"{confirm}>{}<button class=\"btn {class}\">{label}</button></form>",
            c.id,
            hidden_csrf(token)
        )
    };
    match viewer {
        None => format!(
            "<a class=\"btn\" href=\"/login\">{}</a>",
            if c.open { trh("Sign in to join") } else { trh("Sign in to ask") }
        ),
        Some(me) if c.owner_id == Some(me) => {
            format!("<span class=\"bf-club-role\">{tr1}</span>", tr1 = trh("You run this club"))
        }
        Some(_) if c.joined => form("leave", &trh("Leave"), "", Some(&tr("Leave this club?"))),
        Some(_) if c.open || c.invited => form("join", &trh("Join"), "primary", None),
        Some(_) if c.requested => {
            format!(
                "<span class=\"bf-club-role\">{tr1}</span> {}",
                form("withdraw", &trh("Withdraw"), "link", None),
                tr1 = trh("Request sent")
            )
        }
        Some(_) => form("request", &trh("Ask to join"), "primary", None),
    }
}

fn card(
    c: &clubs::Club,
    viewer: Option<i64>,
    token: &str,
    requests: &[(i64, i64, String)],
    people: &[(i64, String)],
) -> String {
    let name = esc(&c.name);
    let visible = c.open || c.joined || c.owner_id == viewer && viewer.is_some();
    let title =
        if visible { format!("<a href=\"{}\">{name}</a>", forum_path(c.node_id, &c.slug)) } else { name.clone() };
    let badge = if c.open {
        format!("<span class=\"bf-club-badge\">{tr1}</span>", tr1 = trh("Open"))
    } else {
        format!("<span class=\"bf-club-badge is-closed\">{tr1}</span>", tr1 = trh("Closed"))
    };
    let badge = if c.archived {
        format!("{badge} <span class=\"bf-club-badge is-closed\">{tr1}</span>", tr1 = trh("Archived"))
    } else {
        badge
    };
    let role = if viewer.is_some() && c.owner_id == viewer {
        format!(" <span class=\"bf-club-badge is-you\">{tr1}</span>", tr1 = trh("Owner"))
    } else if c.joined {
        format!(" <span class=\"bf-club-badge is-you\">{tr1}</span>", tr1 = trh("Member"))
    } else {
        String::new()
    };
    let description = if c.description_html.trim().is_empty() {
        String::new()
    } else {
        format!("<div class=\"bf-club-desc\">{}</div>", c.description_html)
    };
    let members = esc(&lang().n("count.members", c.members));
    let topics = esc(&lang().n("count.topics", c.topic_count));
    let active = c.last_activity_at.map_or_else(
        || format!("<li>{tr1}</li>", tr1 = trh("No posts yet")),
        |t| format!("<li>{}</li>", trf("Active {when}", &[("when", &crate::http::render::ago(t).to_string())])),
    );
    let owner = c
        .owner
        .as_deref()
        .map(|o| {
            format!(
                "<li>{tr1} <a href=\"{}\">{}</a></li>",
                esc(&bf_core::slug::profile_path(c.owner_id.unwrap_or_default(), o)),
                esc(o),
                tr1 = trh("Run by")
            )
        })
        .unwrap_or_default();
    // The owner's tools stay folded on their own card.
    let mut manage = String::new();
    if viewer.is_some() && c.owner_id == viewer {
        let waiting: Vec<&(i64, i64, String)> = requests.iter().filter(|(club, _, _)| *club == c.id).collect();
        let mut req = String::new();
        for (_, user, who) in &waiting {
            req.push_str(&format!(
                "<li><span>{}</span><form method=\"post\" action=\"/clubs/{}/requests/{user}\" class=\"inline\">{}<button class=\"btn primary\" name=\"approve\" value=\"1\">{tr1}</button> <button class=\"btn\" name=\"approve\" value=\"0\">{tr2}</button></form></li>",
                user_link(*user, who),
                c.id,
                hidden_csrf(token), tr1 = trh("Approve"), tr2 = trh("Decline")));
        }
        let req = if waiting.is_empty() {
            String::new()
        } else {
            format!("<h4>{tr1}</h4><ul class=\"bf-club-people\">{req}</ul>", tr1 = trh("Waiting to join"))
        };
        let mut ppl = String::new();
        for (uid, who) in people.iter().filter(|(uid, _)| Some(*uid) != c.owner_id) {
            ppl.push_str(&format!(
                "<li><span>{}</span><form method=\"post\" action=\"/clubs/{}/members/{uid}/remove\" class=\"inline\" data-confirm=\"{}\">{}<button class=\"btn\" name=\"ban\" value=\"0\">{tr1}</button> <button class=\"btn\" name=\"ban\" value=\"1\">{tr2}</button></form></li>",
                user_link(*uid, who),
                c.id,
                esc(&tr("Remove this member from the club?")),
                hidden_csrf(token),
                tr1 = trh("Remove"),
                tr2 = trh("Ban")
            ));
        }
        let ppl = if ppl.is_empty() {
            String::new()
        } else {
            format!("<h4>{tr1}</h4><ul class=\"bf-club-people\">{ppl}</ul>", tr1 = trh("Members"))
        };
        let own = format!(
            "<form method=\"post\" action=\"/clubs/{id}/transfer\" class=\"bf-club-inline-form\" data-confirm=\"{c1}\">{h}<label class=\"bf-field\"><input name=\"username\" placeholder=\" \" autocomplete=\"off\" required><span class=\"bf-field-label\">{t1}</span></label><button class=\"btn\">{t2}</button></form>\
             <div class=\"bf-club-actions\"><form method=\"post\" action=\"/clubs/{id}/archive\" class=\"inline\">{h}<input type=\"hidden\" name=\"archived\" value=\"{av}\"><button class=\"btn\">{t3}</button></form> <form method=\"post\" action=\"/clubs/{id}/delete\" class=\"inline\" data-confirm=\"{c2}\">{h}<button class=\"btn\">{t4}</button></form></div>",
            id = c.id,
            h = hidden_csrf(token),
            av = if c.archived { "0" } else { "1" },
            c1 = esc(&tr("Hand this club over? You will no longer run it.")),
            c2 = esc(&tr("Delete this club for good?")),
            t1 = trh("Hand over to member"),
            t2 = trh("Hand over"),
            t3 = if c.archived { trh("Reopen club") } else { trh("Archive club") },
            t4 = trh("Delete club"),
        );
        manage = format!(
            "<details class=\"bf-club-manage bf-disclosure\"{}><summary>{trm}{}</summary>{req}{ppl}
             <form method=\"post\" action=\"/clubs/{id}/invite\" class=\"bf-club-inline-form\">{h}<label class=\"bf-field\"><input name=\"username\" placeholder=\" \" autocomplete=\"off\" required><span class=\"bf-field-label\">{tr1}</span></label><button class=\"btn\">{tr2}</button></form>
             <form method=\"post\" action=\"/clubs/{id}/describe\" class=\"stack\">{h}<label class=\"bf-field\"><textarea name=\"description\" rows=\"2\" maxlength=\"300\" placeholder=\" \">{desc}</textarea><span class=\"bf-field-label\">{tr3}</span></label><button class=\"btn\">{tr4}</button></form>{own}</details>",
            if waiting.is_empty() { "" } else { " open" },
            if waiting.is_empty() { String::new() } else { format!(" <span class=\"bf-club-count\">{}</span>", waiting.len()) },
            id = c.id,
            h = hidden_csrf(token),
            trm = trh("Manage"),
            desc = esc(&c.description_md), tr1 = trh("Invite by username"), tr2 = trh("Invite"), tr3 = trh("What the club is about"), tr4 = trh("Save description"));
    }
    format!(
        "<li class=\"bf-box bf-club-card\" id=\"club-{id}\"><div class=\"bf-club-top\">{mark}<div class=\"bf-club-name\"><h3>{title}</h3><div>{badge}{role}</div></div></div>{description}<ul class=\"bf-club-stats small muted\"><li>{members}</li><li>{topics}</li>{active}{owner}</ul><div class=\"bf-club-action\">{act}</div>{manage}</li>",
        id = c.id,
        mark = mark(&c.name),
        act = action(c, viewer, token),
    )
}

async fn list(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<ListQuery>,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    use crate::http::ui::{Shell, begin, finish};
    state.require_feature("clubs_enabled")?;
    let pool = state.db()?;
    let viewer = v.user.as_ref().map(|u| u.id);
    let rows = clubs::clubs(pool, viewer.map(bf_core::ids::UserId::get)).await.db()?;
    let requests = match viewer {
        Some(id) => clubs::club_requests(pool, id.get()).await.db()?,
        None => vec![],
    };
    let ctx = begin(&state, &v);
    let token = ctx.token.clone();
    let mine_only = q.show == "mine" && viewer.is_some();
    let needle = q.q.trim().to_lowercase();
    let shown: Vec<&clubs::Club> = rows
        .iter()
        .filter(|c| !mine_only || c.joined || (viewer.is_some() && c.owner_id == viewer.map(bf_core::ids::UserId::get)))
        .filter(|c| !c.archived || c.joined || (viewer.is_some() && c.owner_id == viewer.map(bf_core::ids::UserId::get)))
        .filter(|c| needle.is_empty() || c.name.to_lowercase().contains(&needle))
        .collect();

    let mut html = String::new();
    // Invitations come first: they are waiting for an answer.
    html.push_str(&invitations_html(&rows, &token));
    if !requests.is_empty() {
        html.push_str(&format!(
            "<p class=\"bf-notice\">{} {tr3} <strong>{tr1}</strong> {tr2}</p>",
            bf_core::html::esc(&lang().n("clubs.waiting", requests.len())),
            tr3 = trh("Open"),
            tr1 = trh("Manage"),
            tr2 = trh("on the club to answer.")
        ));
    }
    html.push_str(&club_bar_html(viewer.is_some(), mine_only, &q.q));
    if shown.is_empty() {
        let empty = if !needle.is_empty() {
            trh("No club matches that name.")
        } else if mine_only {
            trh("You're not in any club yet. Join one below the tabs, or start your own.")
        } else {
            trh("No clubs yet. Start the first one.")
        };
        html.push_str(&format!("<p class=\"bf-box block muted bf-club-empty\">{empty}</p>"));
    } else {
        html.push_str("<ul class=\"bf-club-grid\">");
        for c in shown {
            // Only owners see their members: the list is theirs to manage.
            let people = if c.joined && viewer.is_some() && c.owner_id == viewer.map(bf_core::ids::UserId::get) {
                clubs::club_members(pool, c.group_id, c.owner_id, 100).await.db()?
            } else {
                vec![]
            };
            html.push_str(&card(c, viewer.map(bf_core::ids::UserId::get), &token, &requests, &people));
        }
        html.push_str("</ul>");
    }
    let (actions, start_notice) = start_club_html(&state, &v, pool, &token).await?;
    html.push_str(&start_notice);
    let shell = Shell {
        title: v.t().msg("Clubs"),
        intro: format!("<p>{tr1}</p>", tr1 = trh("Small communities inside the forum, each with its own forum and members, run by the member who started it.")),
        actions,
        canonical: Some("/clubs".into()),
        noindex: mine_only || !needle.is_empty(),
        ..Shell::default()
    }
    .with_area(&state, "clubs", edit)
    .await;
    Ok(finish(&state, &v, ctx, shell, html))
}

/// Invitations come first on the page: they are waiting for an answer.
fn invitations_html(rows: &[clubs::Club], token: &str) -> String {
    let mut html = String::new();
    let invites: Vec<&clubs::Club> = rows.iter().filter(|c| c.invited && !c.joined).collect();
    if !invites.is_empty() {
        html.push_str(&format!(
            "<section class=\"bf-box bf-club-notice\" aria-label=\"{tr2}\"><h2>{tr1}</h2><ul>",
            tr1 = trh("You're invited"),
            tr2 = trh("Invitations")
        ));
        for c in invites {
            html.push_str(&format!(
                "<li>{}<strong>{}</strong><span class=\"bf-club-notice-actions\"><form method=\"post\" action=\"/clubs/{id}/join\" class=\"inline\">{h}<button class=\"btn primary\">{tr1}</button></form> <form method=\"post\" action=\"/clubs/{id}/decline\" class=\"inline\">{h}<button class=\"btn\">{tr2}</button></form></span></li>",
                mark(&c.name),
                esc(&c.name),
                id = c.id,
                h = hidden_csrf(token), tr1 = trh("Join"), tr2 = trh("Decline")));
        }
        html.push_str("</ul></section>");
    }
    html
}

/// The "All clubs / My clubs" tabs and the search box.
fn club_bar_html(signed_in: bool, mine_only: bool, query: &str) -> String {
    let tab = |key: &str, label: &str| {
        let current = (key == "mine") == mine_only;
        format!(
            "<a href=\"/clubs{}\"{}>{label}</a>",
            if key == "mine" { "?show=mine" } else { "" },
            if current { " aria-current=\"page\"" } else { "" }
        )
    };
    format!(
        "<div class=\"bf-club-bar\">{}<form method=\"get\" action=\"/clubs\" role=\"search\" class=\"bf-club-search\">{}<input type=\"search\" name=\"q\" value=\"{}\" placeholder=\"{tr1}\" aria-label=\"{tr2}\"></form></div>",
        if signed_in {
            format!(
                "<nav class=\"bf-segments small\" aria-label=\"{}\">{}{}</nav>",
                trh("Show"),
                tab("all", &trh("All clubs")),
                tab("mine", &trh("My clubs"))
            )
        } else {
            String::new()
        },
        if mine_only { "<input type=\"hidden\" name=\"show\" value=\"mine\">" } else { "" },
        esc(query.trim()),
        tr1 = trh("Find a club"),
        tr2 = trh("Find a club")
    )
}

/// The "start a club" dialog for those who may, or the reason they may not: (header actions, notice).
async fn start_club_html(
    state: &AppState,
    v: &Viewer,
    pool: &sqlx::PgPool,
    token: &str,
) -> Result<(String, String), AppError> {
    let mut notice = String::new();
    let s = state.settings();
    let mut actions = String::new();
    if let Some(u) = &v.user {
        let owned = clubs::clubs_owned(pool, u.id.get()).await.db()?;
        let limit = s.int("clubs_per_member");
        let level_ok = u.is_admin || i64::from(u.trust_level) >= s.int("club_min_trust_level");
        let room = u.is_admin || owned < limit;
        if level_ok && room && !v.actor.silenced {
            let left = if u.is_admin {
                String::new()
            } else {
                format!(
                    "<p class=\"small muted\">{}</p>",
                    bf_core::html::esc(&trf("You can start {n} more.", &[("n", &(limit - owned).to_string())]))
                )
            };
            actions.push_str(&format!(
                "<button type=\"button\" class=\"btn primary\" data-dialog=\"new-club\">{tr1}</button>
                 <dialog class=\"bf-dialog is-wide\" id=\"new-club\" aria-labelledby=\"new-club-title\">
                 <header class=\"bf-dialog-head\"><h2 id=\"new-club-title\">{tr2}</h2><button type=\"button\" class=\"bf-icon-btn\" data-dialog-close aria-label=\"{tr12}\"><svg class=\"bf-icon\" aria-hidden=\"true\"><use href=\"{sprite}#x\"></use></svg></button></header>
                 <div class=\"bf-dialog-body\">{left}<form method=\"post\" action=\"/clubs\" class=\"stack\">{h}
                 <label class=\"bf-field\"><input name=\"name\" maxlength=\"60\" placeholder=\" \" required><span class=\"bf-field-label\">{tr3}</span></label>
                 <label class=\"bf-field\"><textarea name=\"description\" rows=\"2\" maxlength=\"300\" placeholder=\" \"></textarea><span class=\"bf-field-label\">{tr4}</span></label>
                 <fieldset class=\"bf-club-choice\"><legend>{tr5}</legend>
                   <label><input type=\"radio\" name=\"open\" value=\"1\" checked><span><strong>{tr6}</strong><small>{tr7}</small></span></label>
                   <label><input type=\"radio\" name=\"open\" value=\"0\"><span><strong>{tr8}</strong><small>{tr9}</small></span></label>
                 </fieldset>
                 <div class=\"bf-dialog-actions\"><button type=\"button\" class=\"btn\" data-dialog-close>{tr10}</button><button class=\"btn primary\">{tr11}</button></div></form></div></dialog>",
                h = hidden_csrf(token),
                sprite = bf_web::assets::ICONS.url(), tr1 = trh("Start a club"), tr2 = trh("Start a club"), tr3 = trh("Club name"), tr4 = trh("What it's about (optional)"), tr5 = trh("Who can take part"), tr6 = trh("Open"), tr7 = trh("Anyone can read it and join with one click."), tr8 = trh("Closed"), tr9 = trh("Only members see it. People ask to join, or you invite them."), tr10 = trh("Cancel"), tr11 = trh("Create club"), tr12 = trh("Close")));
        } else if !level_ok {
            let t = v.t();
            let tip = crate::community::trust::needs_text(state, &t, v, "club_min_trust_level").unwrap_or_default();
            actions.push_str(&format!(
                "<span data-tip=\"{}\" data-tip-gate><button type=\"button\" class=\"btn primary\" disabled>{}</button></span>",
                bf_core::html::esc(&tip),
                trh("Start a club")
            ));
            notice.push_str(&format!(
                "<p class=\"small muted bf-club-empty\">{} <a href=\"/trust\">{}</a></p>",
                bf_core::html::esc(&tip),
                trh("See your progress")
            ));
        } else if !room {
            notice.push_str(&format!(
                "<p class=\"small muted bf-club-empty\">{tr1}</p>",
                tr1 = trh("You run as many clubs as a member can.")
            ));
        }
    } else {
        actions.push_str(&format!("<a class=\"btn\" href=\"/login\">{tr1}</a>", tr1 = trh("Sign in to start a club")));
    }
    Ok((actions, notice))
}

#[derive(Deserialize)]
struct CreateForm {
    csrf: String,
    name: String,
    #[serde(default)]
    open: String,
    #[serde(default)]
    description: String,
}

async fn create(State(state): State<AppState>, v: Viewer, Form(f): Form<CreateForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let s = state.settings();
    if !s.bool("clubs_enabled") {
        return Err(AppError::NotFound);
    }
    if v.actor.silenced || (!user.is_admin && i64::from(user.trust_level) < s.int("club_min_trust_level")) {
        return Err(AppError::Forbidden);
    }
    // A quick refusal; `create_now` repeats the check under a lock, where it cannot be raced.
    let cap = (!user.is_admin).then(|| s.int("clubs_per_member"));
    if let Some(c) = cap
        && clubs::clubs_owned(state.db()?, user.id.get()).await.db()? >= c
    {
        return Err(AppError::Validation("You have started as many clubs as a member can.".into()));
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let checked = crate::moderation::gate::words(&state, &v, f.name.trim()).await?;
    let name = checked.text.as_str();
    let slug = slugify(name);
    if name.is_empty() || name.chars().count() > 60 || slug.is_empty() {
        return Err(AppError::Validation("A club needs a name of up to 60 characters.".into()));
    }
    let open = f.open == "1";
    let about = crate::moderation::gate::words(&state, &v, f.description.trim()).await?;
    if about.text.chars().count() > 300 {
        return Err(AppError::Validation("Keep the description under 300 characters.".into()));
    }
    if checked.held || about.held {
        crate::moderation::held::hold(
            &state,
            user.id.get(),
            "club",
            None,
            json!({ "name": name, "open": open, "description": about.text }),
        )
        .await?;
        return Ok(crate::moderation::held::held_page(&state, &v));
    }
    let id = create_now_capped(&state, user.id.get(), name, open, cap).await?;
    if !about.text.is_empty() {
        describe_now(&state, id, &about.text).await?;
    }
    Ok(Redirect::to(&format!("/clubs?show=mine#club-{id}")).into_response())
}

/// Creates a club: its group, forum and permissions, run by its owner. Also used when a held
/// club is approved.
pub(crate) async fn create_now(state: &AppState, owner: i64, name: &str, open: bool) -> Result<i64, AppError> {
    create_now_capped(state, owner, name, open, None).await
}

/// [`create_now`] for a member limited to `cap` clubs. The limit is enforced when the club row is
/// written, under a per-member lock; a refused club leaves no group or forum behind.
async fn create_now_capped(
    state: &AppState,
    owner: i64,
    name: &str,
    open: bool,
    cap: Option<i64>,
) -> Result<i64, AppError> {
    let s = state.settings();
    let slug = slugify(name);
    let pool = state.db()?;
    let group_name = format!("Club: {name}");
    let group = bf_db::admin::members::create_group(pool, &group_name, &format!("club-{slug}"), owner)
        .await
        .map_err(|_| AppError::Validation("That club name is taken.".into()))?;
    let read = Perms::READER;
    let post = read.union(Perms::CREATE).union(Perms::REPLY).union(Perms::ATTACH);
    let staff = post.union(Perms::MODERATE);
    let mut grants = vec![(group, post), (3, staff), (4, Perms::ALL)];
    if open {
        grants.push((1, read));
        grants.push((2, read));
    }
    let parent = Some(s.int("clubs_parent_node")).filter(|p| *p > 0);
    let node =
        bf_db::forum::nodes::create_node(pool, parent, "forum", name, &slug, "", &grants).await.db()?;
    let Some(id) = clubs::create_club(pool, name, node, group, owner, open, cap).await.db()? else {
        // Another request used up the allowance first: take back what was made for this one.
        bf_db::admin::forums::delete_node(pool, node, owner).await.db()?;
        bf_db::admin::members::delete_group(pool, group, owner).await.db()?;
        state.forum.invalidate();
        return Err(AppError::Validation("You have started as many clubs as a member can.".into()));
    };
    // The owner runs their own forum.
    bf_db::admin::forums::set_node_moderators(pool, node, &[owner], owner).await.db()?;
    state.forum.invalidate();
    state.microcache.clear();
    Ok(id)
}

/// A club's description, shown on its card and its forum.
pub(crate) async fn describe_now(state: &AppState, club: i64, text: &str) -> Result<(), AppError> {
    let pool = state.db()?;
    let c = clubs::club(pool, club, None).await.db()?.ok_or(AppError::NotFound)?;
    let html = if text.trim().is_empty() { String::new() } else { format!("<p>{}</p>", esc(text.trim())) };
    clubs::set_description(pool, c.node_id, text.trim(), &html).await.db()?;
    state.forum.invalidate();
    state.microcache.clear();
    Ok(())
}

/// Takes back a request to join a closed club.
async fn withdraw(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let c = clubs::club(pool, id, Some(user.id.get())).await.db()?.ok_or(AppError::NotFound)?;
    clubs::withdraw_request(pool, c.group_id, user.id.get()).await.db()?;
    Ok(Redirect::to(&format!("/clubs#club-{id}")).into_response())
}

/// Turns down an invitation.
async fn decline(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    clubs::clear_club_invite(state.db()?, id, user.id.get()).await.db()?;
    Ok(Redirect::to("/clubs").into_response())
}

#[derive(Deserialize)]
struct DescribeForm {
    csrf: String,
    #[serde(default)]
    description: String,
}

/// The owner rewrites what the club is about.
async fn describe(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<DescribeForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    owner_of(&state, &v, id, false).await?;
    let about = crate::moderation::gate::words(&state, &v, f.description.trim()).await?;
    if about.held {
        return Err(AppError::Validation("That description needs a moderator; please choose other words.".into()));
    }
    if about.text.chars().count() > 300 {
        return Err(AppError::Validation("Keep the description under 300 characters.".into()));
    }
    describe_now(&state, id, &about.text).await?;
    Ok(Redirect::to(&format!("/clubs?show=mine#club-{id}")).into_response())
}

async fn join(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let (user, c) = applicant(&state, &v, id).await?;
    let pool = state.db()?;
    // A closed club needs an invitation; asking to join goes through `request`.
    if !c.open && !clubs::clubs_invited(pool, user.id.get()).await.db()?.contains(&c.id) {
        return Err(AppError::Forbidden);
    }
    clubs::club_membership(pool, c.group_id, user.id.get(), true).await.db()?;
    clubs::clear_club_invite(pool, c.id, user.id.get()).await.db()?;
    state.forum.invalidate();
    Ok(Redirect::to(&format!("/clubs?show=mine#club-{}", c.id)).into_response())
}

/// Asks to join a closed club; its owner hears about it.
async fn request(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let (user, c) = applicant(&state, &v, id).await?;
    let pool = state.db()?;
    if clubs::request_club(pool, c.group_id, user.id.get()).await.db()?
        && let Some(owner) = c.owner_id
    {
        let data = json!({ "by": user.username, "name": c.name, "url": "/clubs" });
        crate::community::notifications::send(&state, user.id.get(), &[owner], "club_request", &data).await;
    }
    Ok(Redirect::to("/clubs").into_response())
}

#[derive(Deserialize)]
struct ClubInviteForm {
    csrf: String,
    username: String,
}

/// The owner invites a member, which also lets them into a closed club.
async fn invite(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<ClubInviteForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let (user, c) = owner_of(&state, &v, id, false).await?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let pool = state.db()?;
    if c.archived {
        return Err(AppError::Validation("This club is archived.".into()));
    }
    CLUB_INVITES_BY_USER.enforce(&format!("club-invite:{}", user.id))?;
    let person = member_named(pool, &f.username).await?;
    crate::community::messages::check_recipients(&state, user.id.get(), &[person]).await?;
    if clubs::is_banned(pool, c.id, person).await.db()? {
        return Err(AppError::Validation("That member is banned from this club.".into()));
    }
    clubs::invite_to_club(pool, c.id, person).await.db()?;
    let data = json!({ "by": user.username, "name": c.name, "url": "/clubs" });
    crate::community::notifications::send(&state, user.id.get(), &[person], "club_invite", &data).await;
    Ok(Redirect::to("/clubs").into_response())
}

#[derive(Deserialize)]
struct ClubDecision {
    csrf: String,
    approve: String,
}

/// The owner answers a request to join.
async fn decide(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, person)): Path<(i64, i64)>,
    Form(f): Form<ClubDecision>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let c = owner_of(&state, &v, id, false).await?.1;
    if c.archived {
        return Err(AppError::Validation("This club is archived.".into()));
    }
    let pool = state.db()?;
    let approve = f.approve == "1";
    if !bf_db::community::groups::decide_group_request(pool, c.group_id, person, approve).await.db()? {
        return Err(AppError::NotFound);
    }
    crate::community::notifications::request_decided(&state, person, c.group_id, approve).await;
    state.forum.invalidate();
    Ok(Redirect::to("/clubs").into_response())
}

/// The club and the member who wants in. An archived club takes no one; neither does it take someone
/// its owner banned.
async fn applicant<'a>(
    state: &AppState,
    v: &'a Viewer,
    id: i64,
) -> Result<(&'a bf_db::accounts::users::User, clubs::Club), AppError> {
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(state, user).map_err(AppError::Validation)?;
    let pool = state.db()?;
    let c = clubs::club(pool, id, Some(user.id.get())).await.db()?.ok_or(AppError::NotFound)?;
    if c.archived {
        return Err(AppError::Validation("This club is archived.".into()));
    }
    if clubs::is_banned(pool, c.id, user.id.get()).await.db()? {
        return Err(AppError::Forbidden);
    }
    Ok((user, c))
}

/// The club and the signed-in member, when this member runs it (`admin_too`: or administers the site).
async fn owner_of<'a>(
    state: &AppState,
    v: &'a Viewer,
    id: i64,
    admin_too: bool,
) -> Result<(&'a bf_db::accounts::users::User, clubs::Club), AppError> {
    let user = v.require_user()?;
    let c = clubs::club(state.db()?, id, Some(user.id.get())).await.db()?.ok_or(AppError::NotFound)?;
    if c.owner_id != Some(user.id.get()) && !(admin_too && user.is_admin) {
        return Err(AppError::Forbidden);
    }
    Ok((user, c))
}

/// The id of the member a form names (`@` and case ignored).
pub(crate) async fn member_named(pool: &sqlx::PgPool, typed: &str) -> Result<i64, AppError> {
    let name = typed.trim().trim_start_matches('@').to_lowercase();
    let ids = bf_db::accounts::users::user_ids_by_names(pool, &[name]).await.db()?;
    ids.first().map(|i| i.get()).ok_or_else(|| AppError::Validation("There is no member by that name.".into()))
}

#[derive(Deserialize)]
struct RemoveForm {
    csrf: String,
    #[serde(default)]
    ban: String,
}

/// The owner removes a member, or bans them so they cannot ask again.
async fn remove_member(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, person)): Path<(i64, i64)>,
    Form(f): Form<RemoveForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let c = owner_of(&state, &v, id, true).await?.1;
    if c.owner_id == Some(person) {
        return Err(AppError::Validation("The owner cannot be removed from their own club.".into()));
    }
    clubs::remove_member(state.db()?, c.id, c.group_id, person, f.ban == "1").await.db()?;
    state.forum.invalidate();
    Ok(Redirect::to(&format!("/clubs?show=mine#club-{id}")).into_response())
}

#[derive(Deserialize)]
struct TransferForm {
    csrf: String,
    username: String,
}

/// The owner hands the club to another member.
async fn transfer(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<TransferForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let c = owner_of(&state, &v, id, true).await?.1;
    let pool = state.db()?;
    let person = member_named(pool, &f.username).await?;
    if !clubs::transfer_club(pool, c.id, c.node_id, c.group_id, person).await.db()? {
        return Err(AppError::Validation("Only a member of the club can take it over.".into()));
    }
    state.forum.invalidate();
    state.microcache.clear();
    Ok(Redirect::to(&format!("/clubs#club-{id}")).into_response())
}

#[derive(Deserialize)]
struct ArchiveForm {
    csrf: String,
    archived: String,
}

/// Archives a club, or opens it again.
async fn archive(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<ArchiveForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let c = owner_of(&state, &v, id, true).await?.1;
    clubs::set_archived(state.db()?, c.id, f.archived == "1").await.db()?;
    state.microcache.clear();
    Ok(Redirect::to(&format!("/clubs?show=mine#club-{id}")).into_response())
}

/// Deletes a club whose forum has no topics.
async fn delete(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let c = owner_of(&state, &v, id, true).await?.1;
    let actor = v.require_user()?.id.get();
    if !clubs::delete_club(state.db()?, c.id, actor).await.db()? {
        return Err(AppError::Validation("A club with topics cannot be deleted. Archive it instead.".into()));
    }
    state.forum.invalidate();
    state.microcache.clear();
    Ok(Redirect::to("/clubs").into_response())
}

async fn leave(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    state.require_feature("clubs_enabled")?;
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let c = clubs::club(pool, id, Some(user.id.get())).await.db()?.ok_or(AppError::NotFound)?;
    if c.owner_id == Some(user.id.get()) {
        return Err(AppError::Validation("The owner cannot leave their own club.".into()));
    }
    clubs::club_membership(pool, c.group_id, user.id.get(), false).await.db()?;
    state.forum.invalidate();
    Ok(Redirect::to("/clubs").into_response())
}

async fn manifest(State(state): State<AppState>) -> Response {
    let s = state.settings();
    let name = s.str("site_name").to_owned();
    let mut doc = json!({ "name": name, "short_name": name, "start_url": "/", "scope": "/", "display": "standalone" });
    // The installed app's icon: the app icon if set, else the favicon.
    let icon = Some(s.str("large_icon_url"))
        .filter(|i| !i.is_empty())
        .or(Some(s.str("favicon_url")).filter(|i| !i.is_empty()));
    if let Some(src) = icon {
        doc["icons"] = json!([{ "src": src, "sizes": "any", "purpose": "any" }]);
    }
    ([(header::CONTENT_TYPE, "application/manifest+json")], doc.to_string()).into_response()
}

/// Browsers ask for /favicon.ico whatever the page says: send them to the uploaded one.
async fn favicon(State(state): State<AppState>) -> Response {
    let s = state.settings();
    let src = [s.str("favicon_url"), s.str("large_icon_url")].into_iter().find(|i| !i.is_empty()).map(str::to_owned);
    match src {
        Some(src) => {
            (StatusCode::FOUND, [(header::LOCATION, src), (header::CACHE_CONTROL, "public, max-age=3600".to_owned())])
                .into_response()
        }
        // No icon set: an empty answer the browser remembers, so it stops asking and logs no error.
        None => (StatusCode::NO_CONTENT, [(header::CACHE_CONTROL, "public, max-age=3600")]).into_response(),
    }
}

/// Installable and readable offline; see `sw.js`. The strings follow the visitor's language.
async fn service_worker(v: Viewer) -> Response {
    let t = bf_core::i18n::T::new(v.locale);
    let strings = json!({
        "notice": t.t("offline.notice"),
        "heading": t.t("offline.heading"),
        "saved": t.t("offline.saved"),
        "none": t.t("offline.none"),
        "not_saved": t.t("offline.not_saved"),
        "not_sent": t.t("offline.not_sent"),
        "lang": t.locale.to_string(),
        "dir": t.dir(),
    });
    let body = include_str!("../../assets/sw.js").replace("__BF_STRINGS__", &strings.to_string());
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], body)
        .into_response()
}
