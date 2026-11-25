//! Live topics: a moderator or the topic's author turns a topic into a time-boxed Q&A
//! with named hosts. Members send questions (approved first when the session is moderated) and
//! vote for them; hosts answer in replies and mark questions answered. Changes reach open pages
//! through the topic's live channel, which the topic page already listens to.

use crate::http::i18n::{trf, trh};
use crate::http::render::DbResult;
use crate::http::ui::hidden_csrf;
use crate::http::ui::{Shell, begin, finish, user_link, when};
use crate::http::viewer::{CsrfForm, Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::i18n::en;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::forum::live as db;
use chrono::{NaiveDateTime, Utc};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/live", get(list))
        .route("/t/{id}/live", post(save))
        .route("/t/{id}/live/end", post(end))
        .route("/t/{id}/live/remove", post(remove))
        .route("/t/{id}/live/ask", post(ask))
        .route("/live/questions/{qid}/{action}", post(act))
}

fn enabled(state: &AppState) -> bool {
    state.settings().bool("live_topics_enabled")
}

/// The topic, if the viewer may read it: (node, title, slug, author).
async fn topic(state: &AppState, v: &Viewer, id: i64) -> Result<(i64, String, String, Option<i64>, bool), AppError> {
    let pool = state.db()?;
    let row: Option<(i64, String, String, Option<i64>, bool)> =
        sqlx::query_as("SELECT node_id, title, slug, author_id, locked FROM items WHERE id = $1 AND state = 'visible'")
            .bind(id)
            .fetch_optional(pool)
            .await
            .db()?;
    let t = row.ok_or(AppError::NotFound)?;
    let tree = state.forum.get(pool).await?;
    if !tree.perms(&v.actor, t.0).has(Perms::READ) {
        return Err(AppError::NotFound);
    }
    Ok(t)
}

async fn moderates(state: &AppState, v: &Viewer, node: i64) -> Result<bool, AppError> {
    let tree = state.forum.get(state.db()?).await?;
    Ok(tree.perms(&v.actor, node).has(Perms::MODERATE) || is_staff(v))
}

/// The live panel above the posts: the session, its questions, and the forms the viewer may use.
pub(crate) async fn live_html(
    state: &AppState,
    v: &Viewer,
    item: i64,
    node: i64,
    author: Option<i64>,
    csrf: &str,
) -> Result<String, AppError> {
    if !enabled(state) {
        return Ok(String::new());
    }
    let pool = state.db()?;
    let me = v.user.as_ref().map(|u| u.id);
    let staff = moderates(state, v, node).await?;
    let manage = staff || (me.is_some() && me == author.map(Into::into));
    let token = esc(csrf);
    let csrf_field = hidden_csrf(&token);
    let Some(s) = db::session(pool, item).await.db()? else {
        if !manage {
            return Ok(String::new());
        }
        return Ok(format!(
            "<details class=\"bf-box block bf-live-setup bf-disclosure\"><summary>{tr1}</summary>{}</details>",
            setup_form(item, &token, None),
            tr1 = trh("Host a live Q&A in this topic")
        ));
    };
    let host = me.is_some_and(|m| s.hosts.contains(&m.get()));
    let decides = staff || host;
    let now = Utc::now();
    let (status, class) = if now < s.starts_at {
        (trf("Starts {when}", &[("when", &when(s.starts_at))]), "upcoming")
    } else if now < s.ends_at {
        (trf("Live now, until {time}", &[("time", &bf_web::ago::Ago::stamp(s.ends_at).to_string())]), "live")
    } else {
        (trf("Ended {when}", &[("when", &when(s.ends_at))]), "ended")
    };
    let hosts: Vec<String> = s.host_names.iter().map(|(_, n)| user_link(Some(n))).collect();
    let questions = db::questions(pool, item, me.map(bf_core::ids::UserId::get), decides).await.db()?;
    let mut html = format!(
        "<section class=\"bf-box bf-live bf-live-{class}\" id=\"live\"><div class=\"bf-box-head\"><h2>{tr1}</h2><span class=\"small\"><strong>{status}</strong> · {}</span></div><ol class=\"bf-live-questions\">",
        trf(
            "hosted by {hosts}",
            &[("hosts", &if hosts.is_empty() { trh("the topic's author") } else { hosts.join(", ") })]
        ),
        tr1 = trh("Live Q&A")
    );
    for q in &questions {
        let mut tools = String::new();
        if v.user.is_some() && q.status == "approved" && now < s.ends_at {
            tools.push_str(&format!(
                "<form method=\"post\" action=\"/live/questions/{}/vote\" class=\"inline\">{csrf_field}<button class=\"link\" aria-pressed=\"{}\">▲ {}</button></form>",
                q.id, q.voted, q.votes
            ));
        } else {
            tools.push_str(&format!("<span class=\"small muted\">▲ {}</span>", q.votes));
        }
        if decides {
            let actions: &[(&str, &str)] = match q.status.as_str() {
                "pending" => &[("approve", en("Approve")), ("reject", en("Reject"))],
                "approved" => &[("answered", en("Mark answered")), ("reject", en("Reject"))],
                "rejected" => &[("approve", en("Approve"))],
                _ => &[("approve", en("Reopen"))],
            };
            for (a, label) in actions {
                let label = trh(label);
                tools.push_str(&format!(
                    " <form method=\"post\" action=\"/live/questions/{}/{a}\" class=\"inline\">{csrf_field}<button class=\"link\">{label}</button></form>",
                    q.id
                ));
            }
        }
        let badge = match q.status.as_str() {
            "pending" => format!(" <span class=\"bf-badge\">{tr1}</span>", tr1 = trh("Waiting for approval")),
            "rejected" => format!(" <span class=\"bf-badge\">{tr1}</span>", tr1 = trh("Rejected")),
            "answered" => match q.answer_post {
                Some(n) => {
                    format!(" <a class=\"bf-badge bf-badge-ok\" href=\"#post-{n}\">{tr1}</a>", tr1 = trh("Answered"))
                }
                None => format!(" <span class=\"bf-badge bf-badge-ok\">{tr1}</span>", tr1 = trh("Answered")),
            },
            _ => String::new(),
        };
        html.push_str(&format!(
            "<li class=\"bf-live-q bf-live-q-{}\"><div class=\"bf-live-votes\">{tools}</div><div><p>{}</p><p class=\"small muted\">{} · {}{badge}</p></div></li>",
            esc(&q.status),
            esc(&q.body),
            user_link(q.author.as_deref()),
            when(q.created_at)
        ));
    }
    if questions.is_empty() {
        html.push_str(&format!(
            "<li>{}</li>",
            crate::community::people::empty("circle-help", &crate::http::i18n::tr("No questions yet."))
        ));
    }
    html.push_str("</ol>");
    if now < s.ends_at {
        if v.user.is_some() {
            html.push_str(&format!(
                "<form method=\"post\" action=\"/t/{item}/live/ask\" class=\"row-form bf-live-ask\">{csrf_field}<input name=\"body\" maxlength=\"500\" required aria-label=\"{tr2}\" placeholder=\"{tr3}\"> <button class=\"btn primary\">{tr1}</button></form>{}",
                if s.moderated && !decides { format!("<p class=\"small muted\">{}</p>", trh("Questions appear once a host or moderator approves them.")) } else { String::new() }, tr1 = trh("Ask"), tr2 = trh("Your question"), tr3 = trh("Ask a question")));
        } else {
            html.push_str(&format!(
                "<p class=\"small muted\"><a href=\"/login\">{tr1}</a> {tr2}</p>",
                tr1 = trh("Sign in"),
                tr2 = trh("to ask a question.")
            ));
        }
    }
    if manage {
        html.push_str(&format!(
            "<details class=\"block bf-disclosure is-small\"><summary>{tr1}</summary>{}<p>{}<form method=\"post\" action=\"/t/{item}/live/remove\" class=\"inline\" data-confirm=\"{confirm}\">{csrf_field}<button class=\"btn danger\">{tr2}</button></form></p></details>",
            setup_form(item, &token, Some(&s)),
            if now < s.ends_at {
                format!("<form method=\"post\" action=\"/t/{item}/live/end\" class=\"inline\">{csrf_field}<button class=\"btn\">{}</button></form> ", trh("End now"))
            } else {
                String::new()
            }, tr1 = trh("Session settings"), tr2 = trh("Remove"), confirm = trh("Remove the live session and all its questions?")));
    }
    html.push_str("</section>");
    Ok(html)
}

fn setup_form(item: i64, token: &str, s: Option<&db::Session>) -> String {
    let csrf_field = hidden_csrf(token);
    let fmt = |t: chrono::DateTime<Utc>| t.format("%Y-%m-%dT%H:%M").to_string();
    let start = s.map_or_else(|| fmt(Utc::now() + chrono::Duration::hours(1)), |s| fmt(s.starts_at));
    let end = s.map_or_else(|| fmt(Utc::now() + chrono::Duration::hours(2)), |s| fmt(s.ends_at));
    let hosts =
        s.map(|s| s.host_names.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let moderated = s.is_none_or(|s| s.moderated);
    format!(
        "<form method=\"post\" action=\"/t/{item}/live\" class=\"stack\">{csrf_field}<div class=\"row\"><label class=\"bf-field\"><input type=\"datetime-local\" name=\"starts\" value=\"{start}\" required placeholder=\" \"><span class=\"bf-field-label\">{tr1}</span></label><label class=\"bf-field\"><input type=\"datetime-local\" name=\"ends\" value=\"{end}\" required placeholder=\" \"><span class=\"bf-field-label\">{tr2}</span></label></div><label class=\"bf-field\"><input name=\"hosts\" value=\"{}\" placeholder=\" \" data-people-input><span class=\"bf-field-label\">{tr3}</span></label><label><input type=\"checkbox\" name=\"moderated\" value=\"1\"{}> {tr4}</label><button class=\"btn primary\">{tr5}</button></form>",
        esc(&hosts),
        if moderated { " checked" } else { "" },
        tr1 = trh("Starts (UTC)"),
        tr2 = trh("Ends (UTC)"),
        tr3 = trh("Hosts (usernames, comma separated; empty for the topic's author)"),
        tr4 = trh("Approve questions before they show"),
        tr5 = trh("Save session")
    )
}

#[derive(Deserialize)]
struct SessionForm {
    csrf: String,
    starts: String,
    ends: String,
    #[serde(default)]
    hosts: String,
    #[serde(default)]
    moderated: String,
}

async fn save(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<SessionForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let user = v.require_user()?;
    let (node, _, slug, author, _) = topic(&state, &v, id).await?;
    if !(moderates(&state, &v, node).await? || author == Some(user.id.get())) {
        return Err(AppError::Forbidden);
    }
    let parse = |s: &str| NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%dT%H:%M").map(|n| n.and_utc());
    let (Ok(starts), Ok(ends)) = (parse(&f.starts), parse(&f.ends)) else {
        return Err(AppError::Validation("Give a start and an end time.".into()));
    };
    if ends <= starts || ends - starts > chrono::Duration::days(14) {
        return Err(AppError::Validation("A session ends after it starts, and lasts at most two weeks.".into()));
    }
    let pool = state.db()?;
    let names: Vec<String> = f
        .hosts
        .split(',')
        .map(|n| n.trim().trim_start_matches('@').to_lowercase())
        .filter(|n| !n.is_empty())
        .take(10)
        .collect();
    let mut hosts =
        if names.is_empty() { vec![] } else { bf_db::accounts::users::user_ids_by_names(pool, &names).await.db()? };
    if hosts.is_empty() {
        hosts.push((author.unwrap_or(user.id.get())).into());
    }
    db::save(pool, id, starts, ends, &bf_core::ids::raw(&hosts), f.moderated == "1", user.id.get()).await.db()?;
    state.hub.publish(&format!("item:{id}"), "state", json!({ "action": "live" }));
    state.microcache.clear();
    Ok(Redirect::to(&format!("{}#live", topic_path(id, &slug))).into_response())
}

async fn end(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (node, _, slug, author, _) = topic(&state, &v, id).await?;
    if !(moderates(&state, &v, node).await? || author == Some(user.id.get())) {
        return Err(AppError::Forbidden);
    }
    db::end(state.db()?, id).await.db()?;
    state.hub.publish(&format!("item:{id}"), "state", json!({ "action": "live" }));
    state.microcache.clear();
    Ok(Redirect::to(&format!("{}#live", topic_path(id, &slug))).into_response())
}

async fn remove(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (node, _, slug, author, _) = topic(&state, &v, id).await?;
    if !(moderates(&state, &v, node).await? || author == Some(user.id.get())) {
        return Err(AppError::Forbidden);
    }
    db::remove(state.db()?, id).await.db()?;
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(id, &slug)).into_response())
}

/// Questions one member may send to a session within ten minutes.
const MAX_QUESTIONS_IN_TEN_MINUTES: i64 = 5;

#[derive(Deserialize)]
struct AskForm {
    csrf: String,
    body: String,
}

async fn ask(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<AskForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let (node, _, slug, _, locked) = topic(&state, &v, id).await?;
    let pool = state.db()?;
    let s = db::session(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    if Utc::now() >= s.ends_at || locked {
        return Err(AppError::Validation("This session has ended.".into()));
    }
    let body: String = f.body.trim().chars().take(500).collect();
    if body.is_empty() {
        return Err(AppError::Validation("Write your question first.".into()));
    }
    let checked = crate::moderation::gate::words(&state, &v, &body).await?;
    let decides = s.hosts.contains(&user.id.get()) || moderates(&state, &v, node).await?;
    // Hosts and moderators run the session; everyone else gets a few questions at a time.
    if !decides && db::recent_questions(pool, id, user.id.get()).await.db()? >= MAX_QUESTIONS_IN_TEN_MINUTES {
        return Err(AppError::Validation(
            "You have asked several questions in a short time. Please wait a little before asking again.".into(),
        ));
    }
    let approved = decides || (!s.moderated && !checked.held);
    db::ask(pool, id, user.id.get(), &checked.text, approved).await.db()?;
    if !approved {
        let others: Vec<i64> = s.hosts.iter().copied().filter(|h| *h != user.id).collect();
        let data =
            json!({ "url": format!("{}#live", topic_path(id, &slug)), "group": format!("live-{id}"), "count": 1 });
        crate::community::notifications::send(&state, user.id.get(), &others, "review_queue", &data).await;
    }
    state.hub.publish(&format!("item:{id}"), "state", json!({ "action": "live" }));
    Ok(Redirect::to(&format!("{}#live", topic_path(id, &slug))).into_response())
}

async fn act(
    State(state): State<AppState>,
    v: Viewer,
    Path((qid, action)): Path<(i64, String)>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let (item, asker, status) = db::question_item(pool, qid).await.db()?.ok_or(AppError::NotFound)?;
    let (node, _, slug, _, _) = topic(&state, &v, item).await?;
    let s = db::session(pool, item).await.db()?.ok_or(AppError::NotFound)?;
    let decides = s.hosts.contains(&user.id.get()) || moderates(&state, &v, node).await?;
    match action.as_str() {
        "vote" => {
            // The same rules as asking: the feature is on, and the account may write.
            if !enabled(&state) {
                return Err(AppError::NotFound);
            }
            if v.actor.silenced {
                return Err(AppError::Forbidden);
            }
            crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
            // Only what the panel offers a vote on: a question that was approved, while the
            // session runs. One still waiting or rejected is not shown, so it takes no votes.
            if status != "approved" {
                return Err(AppError::NotFound);
            }
            if Utc::now() >= s.ends_at {
                return Err(AppError::Validation("This session has ended.".into()));
            }
            if asker == Some(user.id.get()) {
                return Err(AppError::Validation("You cannot vote for your own question.".into()));
            }
            db::vote(pool, qid, user.id.get()).await.db()?;
        }
        "approve" if decides => db::set_status(pool, qid, "approved", None).await.db()?,
        "reject" if decides => db::set_status(pool, qid, "rejected", None).await.db()?,
        "answered" if decides => {
            let post = db::latest_host_post(pool, item, &s.hosts).await.db()?;
            db::set_status(pool, qid, "answered", post).await.db()?;
            if let Some(a) = asker {
                let url = match post {
                    Some(n) => format!("{}#post-{n}", topic_path(item, &slug)),
                    None => format!("{}#live", topic_path(item, &slug)),
                };
                crate::community::notifications::send(
                    &state,
                    user.id.get(),
                    &[a],
                    "live_answered",
                    &json!({ "by": user.username, "url": url }),
                )
                .await;
            }
        }
        _ => return Err(AppError::Forbidden),
    }
    state.hub.publish(&format!("item:{item}"), "state", json!({ "action": "live" }));
    Ok(Redirect::to(&format!("{}#live", topic_path(item, &slug))).into_response())
}

/// Sessions running now and coming up.
async fn list(
    State(state): State<AppState>,
    v: Viewer,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let rows = db::upcoming(pool, &tree.readable(&v.actor)).await.db()?;
    let ctx = begin(&state, &v);
    let now = Utc::now();
    let mut html = String::from("<section class=\"bf-box\"><ul class=\"bf-rows\">");
    for (id, title, slug, starts, ends) in &rows {
        let when_txt = if now < *starts {
            format!("starts {}", when(*starts))
        } else if now < *ends {
            format!("<strong>{tr1}</strong>", tr1 = trh("live now"))
        } else {
            format!("ended {}", when(*ends))
        };
        html.push_str(&format!(
            "<li><a href=\"{}#live\">{}</a> <span class=\"small muted\">{when_txt}</span></li>",
            esc(&topic_path(*id, slug)),
            esc(title)
        ));
    }
    if rows.is_empty() {
        html.push_str(&format!(
            "<li>{}</li>",
            crate::community::people::empty("calendar", &crate::http::i18n::tr("No live sessions coming up."))
        ));
    }
    html.push_str("</ul></section>");
    let shell = Shell { title: v.t().msg("Live Q&A"), canonical: Some("/live".into()), ..Shell::default() }
        .with_area(&state, "live", edit)
        .await;
    Ok(finish(&state, &v, ctx, shell, html))
}
