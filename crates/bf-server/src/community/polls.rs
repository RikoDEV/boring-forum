//! Polls: several per topic. A poll is a choice (one or several answers), a ranked choice (members
//! order the options, counted by instant runoff) or a rating (a number). It may close at a time,
//! show its voters, keep votes final, hide results until the viewer votes, until it closes or from
//! everyone but moderators, and limit who may vote by group or trust level. The topic's author or a
//! moderator adds, edits, closes and removes polls.

use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::ui::{hidden_csrf, user_link};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::community::polls::{self, Edit, Edited, NewPoll, Poll};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/t/{id}/poll", post(create_poll))
        .route("/t/{id}/poll/{poll}/edit", post(edit_poll))
        .route("/t/{id}/poll/{poll}/closing", post(set_closing))
        .route("/t/{id}/poll/{poll}/delete", post(delete_poll))
        .route("/t/{id}/vote", post(vote))
}

/// The members voting in the polls of a page.
pub(crate) struct Voter {
    pub id: i64,
    pub groups: Vec<i64>,
    pub trust_level: i16,
}

/// A submitted form: repeated names (several options, one rank per option) stay in order.
struct Fields(Vec<(String, String)>);

impl Fields {
    fn get(&self, k: &str) -> &str {
        self.0.iter().find(|(n, _)| n == k).map_or("", |(_, v)| v.as_str())
    }
    fn on(&self, k: &str) -> bool {
        self.get(k) == "1"
    }
}

/// What a create or edit form asks for, checked.
struct Parsed {
    question: String,
    options: Vec<String>,
    kind: String,
    max_choices: i32,
    closes_at: Option<DateTime<Utc>>,
    results: String,
    is_public: bool,
    final_votes: bool,
    min_trust_level: i16,
    groups: Vec<i64>,
}

/// `kind` is fixed for an existing poll. `closes_at` is what to keep when the form leaves it empty
/// (an edit); a new poll passes `None`.
async fn parse(
    state: &AppState,
    v: &Viewer,
    f: &Fields,
    kind: Option<&str>,
    keep_closing: Option<Option<DateTime<Utc>>>,
) -> Result<Parsed, AppError> {
    let user = v.require_user()?;
    let kind = match kind.unwrap_or_else(|| f.get("kind")) {
        "ranked" => "ranked",
        "rating" => "rating",
        _ => "choice",
    };
    let question = crate::moderation::gate::words(state, v, f.get("question").trim()).await?.text;
    if question.is_empty() || question.chars().count() > 200 {
        return Err(AppError::Validation("A poll needs a question of up to 200 characters.".into()));
    }
    let mut options: Vec<String> =
        f.get("options").lines().map(str::trim).filter(|o| !o.is_empty()).map(str::to_owned).collect();
    if kind == "rating" {
        let num = |k: &str, d: i32| f.get(k).trim().parse::<i32>().unwrap_or(d);
        options = polls::rating_options(num("rating_min", 1), num("rating_max", 5), num("rating_step", 1)).ok_or_else(
            || {
                AppError::invalid(
                    "A rating poll runs from a number to a larger one, with {n} values at most.",
                    &[("n", &polls::MAX_OPTIONS)],
                )
            },
        )?;
    } else if !(2..=polls::MAX_OPTIONS).contains(&options.len()) || options.iter().any(|o| o.chars().count() > 100) {
        return Err(AppError::invalid(
            "A poll needs 2 to {n} options of up to 100 characters, one per line.",
            &[("n", &polls::MAX_OPTIONS)],
        ));
    }
    let closes_at = match parse_when(f.get("closes_at"))? {
        When::None => keep_closing.flatten(),
        When::At(t) => Some(t),
        When::Local(local) => {
            let zone = bf_db::accounts::preferences::profile_fields(state.db()?, user.id)
                .await
                .db()?
                .timezone
                .filter(|z| !z.is_empty())
                .unwrap_or_else(|| "UTC".into());
            let at = polls::local_to_utc(state.db()?, local, &zone).await.db()?;
            Some(at.unwrap_or_else(|| local.and_utc()))
        }
    };
    if !f.get("closes_at").trim().is_empty() && closes_at.is_some_and(|c| c <= Utc::now()) {
        return Err(AppError::Validation("The closing time must be in the future.".into()));
    }
    let results = match f.get("results") {
        r @ ("voted" | "closed" | "staff") => r,
        _ => "always",
    };
    let names: Vec<String> =
        f.get("groups").split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).collect();
    if names.len() > 10 {
        return Err(AppError::Validation("Name up to 10 groups.".into()));
    }
    let (groups, unknown) = polls::groups_by_name(state.db()?, &names).await.db()?;
    if let Some(name) = unknown.first() {
        return Err(AppError::invalid("There is no group called {name}.", &[("name", name)]));
    }
    Ok(Parsed {
        question,
        options,
        kind: kind.to_owned(),
        max_choices: f.get("max_choices").trim().parse().unwrap_or(1),
        closes_at,
        results: results.to_owned(),
        is_public: f.on("public"),
        final_votes: f.on("final"),
        min_trust_level: f.get("min_trust_level").trim().parse::<i16>().unwrap_or(0).clamp(0, bf_core::trust::LIMIT),
        groups,
    })
}

/// The topic's author or a moderator may add polls, up to `polls::MAX_PER_TOPIC`. A poll may
/// belong to one of the author's own replies (a moderator's to any).
async fn create_poll(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let f = Fields(fields);
    v.check_csrf(f.get("csrf"))?;
    let user = v.require_user()?;
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) && (t.author_id != Some(user.id.get()) || v.actor.silenced) {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    // A reply number puts the poll with that reply; the first post is the topic itself.
    let post_number = match f.get("post_number").trim() {
        "" => None,
        n => Some(n.parse::<i32>().map_err(|_| AppError::Validation("That is not a reply number.".into()))?),
    }
    .filter(|n| *n > 1);
    if let Some(n) = post_number {
        let author = polls::post_author(state.db()?, id, n)
            .await
            .db()?
            .ok_or_else(|| AppError::Validation("There is no such reply.".into()))?;
        if !perms.has(Perms::MODERATE) && author != Some(user.id.get()) {
            return Err(AppError::Forbidden);
        }
    }
    let p = parse(&state, &v, &f, None, None).await?;
    let new = NewPoll {
        question: &p.question,
        options: &p.options,
        max_choices: p.max_choices,
        closes_at: p.closes_at,
        results: &p.results,
        kind: &p.kind,
        is_public: p.is_public,
        final_votes: p.final_votes,
        min_trust_level: p.min_trust_level,
        post_number,
        groups: &p.groups,
    };
    if polls::add_poll(state.db()?, id, &new).await.db()?.is_none() {
        return Err(AppError::invalid("A topic can have at most {n} polls.", &[("n", &polls::MAX_PER_TOPIC)]));
    }
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}

/// Saves a poll's settings; see `polls::edit_poll` for what happens to its options.
async fn edit_poll(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, poll)): Path<(i64, i64)>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let f = Fields(fields);
    v.check_csrf(f.get("csrf"))?;
    let user = v.require_user()?;
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) && (t.author_id != Some(user.id.get()) || v.actor.silenced) {
        return Err(AppError::Forbidden);
    }
    let pool = state.db()?;
    let existing = polls::poll(pool, id, poll).await.db()?.ok_or(AppError::NotFound)?;
    let p = parse(&state, &v, &f, Some(&existing.kind), Some(existing.closes_at)).await?;
    let edit = Edit {
        question: &p.question,
        options: &p.options,
        max_choices: p.max_choices,
        closes_at: p.closes_at,
        results: &p.results,
        is_public: p.is_public,
        final_votes: p.final_votes,
        min_trust_level: p.min_trust_level,
        groups: &p.groups,
        confirm_removal: f.on("confirm_removal"),
    };
    match polls::edit_poll(pool, id, poll, &edit).await.db()? {
        Edited::Saved => {}
        Edited::NotFound => return Err(AppError::NotFound),
        Edited::WouldRemoveVotes(n) => {
            return Err(AppError::invalid(
                "Removing those options deletes {n} votes. Tick the box to confirm.",
                &[("n", &n)],
            ));
        }
    }
    state.microcache.clear();
    Ok(Redirect::to(&format!("{}#poll-{poll}", topic_path(t.id, &t.slug))).into_response())
}

/// Closes a poll now (`closed=1`) or opens it again (`closed=0`).
async fn set_closing(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, poll)): Path<(i64, i64)>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let f = Fields(fields);
    v.check_csrf(f.get("csrf"))?;
    let user = v.require_user()?;
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) && (t.author_id != Some(user.id.get()) || v.actor.silenced) {
        return Err(AppError::Forbidden);
    }
    if !polls::set_closed(state.db()?, id, poll, f.get("closed") == "1").await.db()? {
        return Err(AppError::NotFound);
    }
    state.microcache.clear();
    Ok(Redirect::to(&format!("{}#poll-{poll}", topic_path(t.id, &t.slug))).into_response())
}

/// A closing time as submitted.
#[derive(Debug, PartialEq, Eq)]
enum When {
    None,
    /// A full RFC 3339 time: a definite moment.
    At(DateTime<Utc>),
    /// `2026-12-01T18:30` (a `datetime-local` value): a wall-clock time in the member's zone.
    Local(NaiveDateTime),
}

fn parse_when(s: &str) -> Result<When, AppError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(When::None);
    }
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(When::At(t.with_timezone(&Utc)));
    }
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M")
        .map(When::Local)
        .map_err(|_| AppError::Validation("That closing time is not a date and time.".into()))
}

async fn delete_poll(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, poll)): Path<(i64, i64)>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let f = Fields(fields);
    v.check_csrf(f.get("csrf"))?;
    let user = v.require_user()?;
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) && t.author_id != Some(user.id.get()) {
        return Err(AppError::Forbidden);
    }
    if !polls::delete_poll(state.db()?, id, poll).await.db()? {
        return Err(AppError::NotFound);
    }
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}

/// Fields: `csrf`, `poll`, one `option` per choice (repeated for multiple choice), `rank_<option>`
/// for a ranked poll (1 is best, empty leaves the option out), or `withdraw=1` to take the vote back.
async fn vote(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let f = Fields(fields);
    v.check_csrf(f.get("csrf"))?;
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    let pool = state.db()?;
    let poll_id = f.get("poll").parse::<i64>().ok();
    let poll = match poll_id {
        Some(p) => polls::poll(pool, id, p).await.db()?,
        // Older forms carry no poll: the topic's first one.
        None => polls::polls_of(pool, id).await.db()?.into_iter().next(),
    }
    .ok_or(AppError::NotFound)?;
    if poll.closed(t.locked || t.archived, Utc::now()) {
        return Err(AppError::Validation("This poll is closed.".into()));
    }
    let groups: Vec<i64> = v.actor.groups.iter().map(|g| g.0).collect();
    if !poll.may_vote(&groups, user.trust_level) {
        return Err(AppError::Validation("You may not vote in this poll.".into()));
    }
    if !poll.vote_change && polls::has_voted(pool, poll.id, user.id.get()).await.db()? {
        return Err(AppError::Validation("Votes in this poll are final.".into()));
    }
    if f.get("withdraw") == "1" {
        polls::unvote(pool, poll.id, user.id.get()).await.db()?;
        crate::platform::webhooks::fire(
            &state,
            "poll.voted",
            json!({ "user_id": user.id.get(), "poll_id": poll.id, "topic_id": id, "withdrawn": true }),
        );
    } else {
        let options = chosen_options(&f.0, &poll)?;
        let Some(changing) = polls::vote(pool, &poll, &options, user.id.get()).await.db()? else {
            return Err(if options.len() > poll.max_choices as usize {
                AppError::invalid("Choose at most {n} options.", &[("n", &poll.max_choices)])
            } else {
                AppError::Validation("Choose an option.".into())
            });
        };
        crate::platform::webhooks::fire(
            &state,
            "poll.voted",
            json!({ "user_id": user.id.get(), "poll_id": poll.id, "topic_id": id, "options": options, "withdrawn": false }),
        );
        if let (false, Some(author)) = (changing, t.author_id) {
            let data = json!({ "by": user.username, "title": t.title, "url": topic_path(t.id, &t.slug), "group": format!("poll:{id}"), "count": 1 });
            crate::community::notifications::send(&state, user.id.get(), &[author], "poll_vote", &data).await;
        }
    }
    state.microcache.clear();
    Ok(Redirect::to(&format!("{}#poll-{}", topic_path(t.id, &t.slug), poll.id)).into_response())
}

/// The option ids a vote names, in rank order for a ranked poll.
fn chosen_options(fields: &[(String, String)], poll: &Poll) -> Result<Vec<i64>, AppError> {
    if poll.kind != "ranked" {
        return Ok(fields.iter().filter(|(n, _)| n == "option").filter_map(|(_, v)| v.parse().ok()).collect());
    }
    let mut ranked: Vec<(u32, i64)> = fields
        .iter()
        .filter_map(|(n, v)| {
            Some((v.trim().parse::<u32>().ok().filter(|r| *r >= 1)?, n.strip_prefix("rank_")?.parse().ok()?))
        })
        .collect();
    ranked.sort_unstable();
    if ranked.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(AppError::Validation("Give each option its own rank.".into()));
    }
    Ok(ranked.into_iter().map(|(_, id)| id).collect())
}

/// The fields of a poll form, escaped. A new poll's form has the kind and the rating range; an
/// existing poll's has what can still change, with its present values. CSS shows the parts that
/// belong to the chosen kind (`components/poll-form.css`).
pub(crate) fn poll_fields(t: bf_core::i18n::T, edit: Option<(&Poll, &[String])>) -> String {
    let kind = edit.map_or("choice", |(p, _)| p.kind.as_str());
    let value = |s: &str| esc(s);
    let (question, options, max_choices, trust, groups) = match edit {
        Some((p, o)) => (
            value(&p.question),
            value(&o.join("\n")),
            p.max_choices,
            p.min_trust_level,
            value(&p.group_names.join(", ")),
        ),
        None => (String::new(), String::new(), 1, 0, String::new()),
    };
    let selected = |current: &str, v: &str| if current == v { " selected" } else { "" };
    let checked = |on: bool| if on { " checked" } else { "" };
    let results = edit.map_or("always", |(p, _)| p.results.as_str());
    let mut h = String::from("<div class=\"stack bf-poll-fields\">");
    h.push_str(&format!(
        "<label class=\"bf-field\"><input name=\"question\" placeholder=\" \" maxlength=\"200\" value=\"{question}\" required><span class=\"bf-field-label\">{}</span></label>",
        esc(&t.t("poll.question"))
    ));
    if edit.is_none() {
        h.push_str(&format!(
            "<label class=\"bf-field\"><select name=\"kind\"><option value=\"choice\">{}</option><option value=\"ranked\">{}</option><option value=\"rating\">{}</option></select><span class=\"bf-field-label\">{}</span></label>",
            esc(&t.msg("Choose one or more")),
            esc(&t.msg("Rank the options")),
            esc(&t.msg("Rate with a number")),
            esc(&t.msg("Kind of poll"))
        ));
    }
    if kind != "rating" {
        h.push_str(&format!(
            "<label class=\"bf-field bf-poll-options\"><textarea name=\"options\" rows=\"4\" placeholder=\" \">{options}</textarea><span class=\"bf-field-label\">{}</span></label>",
            esc(&t.t("poll.options"))
        ));
    }
    if kind == "choice" {
        h.push_str(&format!(
            "<label class=\"bf-field bf-poll-choice\"><input name=\"max_choices\" type=\"number\" min=\"1\" max=\"20\" value=\"{max_choices}\" placeholder=\" \"><span class=\"bf-field-label\">{}</span></label>",
            esc(&t.t("poll.max_choices"))
        ));
    }
    if edit.is_none() {
        h.push_str(&format!(
            "<div class=\"bf-manage-row bf-poll-rating\"><label class=\"bf-field\"><input name=\"rating_min\" type=\"number\" value=\"1\" placeholder=\" \"><span class=\"bf-field-label\">{}</span></label><label class=\"bf-field\"><input name=\"rating_max\" type=\"number\" value=\"5\" placeholder=\" \"><span class=\"bf-field-label\">{}</span></label><label class=\"bf-field\"><input name=\"rating_step\" type=\"number\" min=\"1\" value=\"1\" placeholder=\" \"><span class=\"bf-field-label\">{}</span></label></div>",
            esc(&t.msg("Rating from")),
            esc(&t.msg("Rating to")),
            esc(&t.msg("Step"))
        ));
    }
    let closing = match edit {
        Some((p, _)) if p.closes_at.is_some() => t.msg("New closing time (empty keeps the present one)"),
        _ => t.msg("Closes at (in your profile time zone, optional)"),
    };
    h.push_str(&format!(
        "<label class=\"bf-field\"><input name=\"closes_at\" type=\"datetime-local\" placeholder=\" \"><span class=\"bf-field-label\">{}</span></label>",
        esc(&closing)
    ));
    h.push_str(&format!(
        "<label class=\"bf-field\"><select name=\"results\"><option value=\"always\"{}>{}</option><option value=\"voted\"{}>{}</option><option value=\"closed\"{}>{}</option><option value=\"staff\"{}>{}</option></select><span class=\"bf-field-label\">{}</span></label>",
        selected(results, "always"),
        esc(&t.t("poll.results_always")),
        selected(results, "voted"),
        esc(&t.t("poll.results_voted")),
        selected(results, "closed"),
        esc(&t.t("poll.results_closed")),
        selected(results, "staff"),
        esc(&t.msg("Only moderators see the results")),
        esc(&t.t("poll.results"))
    ));
    h.push_str(&format!(
        "<label class=\"bf-field\"><input name=\"groups\" placeholder=\" \" value=\"{groups}\"><span class=\"bf-field-label\">{}</span></label>\
         <label class=\"bf-field\"><input name=\"min_trust_level\" type=\"number\" min=\"0\" max=\"{}\" value=\"{trust}\" placeholder=\" \"><span class=\"bf-field-label\">{}</span></label>",
        esc(&t.msg("Groups that may vote (comma-separated)")),
        bf_core::trust::LIMIT,
        esc(&t.msg("Lowest trust level that may vote"))
    ));
    if edit.is_none() {
        h.push_str(&format!(
            "<label class=\"bf-field\"><input name=\"post_number\" type=\"number\" min=\"2\" placeholder=\" \"><span class=\"bf-field-label\">{}</span></label>",
            esc(&t.msg("Belongs to reply number (empty for the topic)"))
        ));
    }
    h.push_str(&format!(
        "<label class=\"check\"><input type=\"checkbox\" name=\"public\" value=\"1\"{}> {}</label>\
         <label class=\"check\"><input type=\"checkbox\" name=\"final\" value=\"1\"{}> {}</label>",
        checked(edit.is_some_and(|(p, _)| p.is_public)),
        esc(&t.msg("Show who voted")),
        checked(edit.is_some_and(|(p, _)| !p.vote_change)),
        esc(&t.msg("Votes are final: nobody can change or withdraw one"))
    ));
    if edit.is_some() && kind != "rating" {
        h.push_str(&format!(
            "<label class=\"check\"><input type=\"checkbox\" name=\"confirm_removal\" value=\"1\"> {}</label>",
            esc(&t.msg("Delete the votes for options I take off the list"))
        ));
    }
    h.push_str("</div>");
    h
}

/// Every poll on a topic page, already escaped. Called only for topics that have one.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn poll_html(
    state: &AppState,
    item: i64,
    viewer: Option<&Voter>,
    csrf: &str,
    locked: bool,
    moderator: bool,
    can_manage: bool,
    t: bf_core::i18n::T,
) -> Result<String, AppError> {
    // The marker stands for the closing time's <time> element, put in after the words are escaped.
    const WHEN: &str = "\u{e000}";
    let pool = state.db()?;
    let all = polls::polls_of(pool, item).await.db()?;
    let options = polls::options_of(pool, item, viewer.map(|v| v.id)).await.db()?;
    let voters =
        if all.iter().any(|p| p.is_public) { polls::voters_of(pool, item, 300).await.db()? } else { Vec::new() };
    let csrf = esc(csrf);
    let csrf_field = hidden_csrf(&csrf);
    let sprite = bf_web::assets::ICONS.url();
    let mut html = String::new();
    let archived: bool = sqlx::query_scalar("SELECT archived FROM items WHERE id = $1")
        .bind(item)
        .fetch_optional(pool)
        .await
        .db()?
        .unwrap_or(false);
    let now = Utc::now();
    for p in &all {
        let rows: Vec<_> = options.iter().filter(|o| o.0 == p.id).collect();
        let closed = p.closed(locked || archived, now);
        let voted = rows.iter().any(|r| r.4);
        let show = match p.results.as_str() {
            "voted" => voted || closed || can_manage,
            "closed" => closed || can_manage,
            "staff" => moderator,
            _ => true,
        };
        let eligible = viewer.is_some_and(|m| p.may_vote(&m.groups, m.trust_level));
        let open = viewer.is_some() && !closed;
        // A vote that is final and cast leaves nothing to submit.
        let can_submit = open && eligible && (p.vote_change || !voted);
        let total: i64 = rows.iter().map(|r| r.3).sum();
        html.push_str(&format!(
            "<section class=\"bf-box block poll\" id=\"poll-{}\"><h2>{}</h2>",
            p.id,
            esc(&p.question)
        ));
        if let Some(n) = p.post_number {
            html.push_str(&format!(
                "<p class=\"small muted\"><a href=\"#post-{n}\">{}</a></p>",
                esc(&t.msgf("From reply {n}", &[("n", &n.to_string())]))
            ));
        }

        let mut meta = if show { vec![t.n("count.voters", p.voters)] } else { Vec::new() };
        match p.kind.as_str() {
            "ranked" => meta.push(t.msg("ranked choice")),
            "rating" => meta.push(t.msg("rating")),
            _ if p.multiple() => meta.push(t.msgf("choose up to {n}", &[("n", &p.max_choices.to_string())])),
            _ => {}
        }
        match (closed, p.closes_at) {
            (true, _) => meta.push(t.msg("closed")),
            (false, Some(c)) => {
                meta.push(
                    t.msgf("closes {when}", &[("when", WHEN)])
                        .replace(WHEN, &format!("\u{e001}{}\u{e002}", Ago::stamp(c))),
                );
            }
            (false, None) => {}
        }
        if p.is_public {
            meta.push(t.msg("votes are public"));
        }
        if !p.vote_change {
            meta.push(t.msg("votes are final"));
        }
        if !p.group_names.is_empty() {
            meta.push(t.msgf("only {groups} may vote", &[("groups", &p.group_names.join(", "))]));
        }
        if p.min_trust_level > 0 {
            meta.push(t.msgf("trust level {n} needed", &[("n", &p.min_trust_level.to_string())]));
        }
        if !show {
            meta.push(match p.results.as_str() {
                "closed" => t.msg("results when it closes"),
                "staff" => t.msg("results are for moderators"),
                _ => t.msg("results after you vote"),
            });
        }

        // Ranked results are counted by instant runoff; the others show each option's votes.
        let ranked = if p.kind == "ranked" && show {
            let ids: Vec<i64> = rows.iter().map(|r| r.1).collect();
            Some(polls::instant_runoff(&polls::ballots(pool, p.id).await.db()?, &ids))
        } else {
            None
        };
        let mine: std::collections::HashMap<i64, i16> = if p.kind == "ranked" && can_submit && voted {
            my_ranks(pool, p.id, viewer.map_or(0, |m| m.id)).await.db()?
        } else {
            std::collections::HashMap::new()
        };
        if can_submit {
            html.push_str(&format!(
                "<form method=\"post\" action=\"/t/{item}/vote\">{csrf_field}<input type=\"hidden\" name=\"poll\" value=\"{}\">",
                p.id
            ));
        }
        let mut order: Vec<&&(i64, i64, String, i64, bool)> = rows.iter().collect();
        if let Some(r) = &ranked {
            order.sort_by_key(|o| r.order.iter().position(|x| *x == o.1).unwrap_or(usize::MAX));
        }
        html.push_str(&format!("<ul class=\"activity{}\">", if p.kind == "rating" { " is-rating" } else { "" }));
        for (rank, o) in order.iter().enumerate() {
            let (_, id, label, votes, chosen) = &***o;
            let input = if can_submit {
                if p.kind.as_str() == "ranked" {
                    let current = mine.get(id).copied();
                    let mut s = format!(
                        "<select name=\"rank_{id}\" aria-label=\"{}\"><option value=\"\">–</option>",
                        esc(&t.msg("Rank"))
                    );
                    for n in 1..=rows.len() {
                        let sel = if current == Some(n as i16) { " selected" } else { "" };
                        s.push_str(&format!("<option value=\"{n}\"{sel}>{n}</option>"));
                    }
                    s.push_str("</select> ");
                    s
                } else {
                    let kind = if p.multiple() { "checkbox" } else { "radio" };
                    let checked = if *chosen { " checked" } else { "" };
                    format!("<input type=\"{kind}\" name=\"option\" value=\"{id}\"{checked}> ")
                }
            } else {
                String::new()
            };
            let count = if let Some(r) = &ranked {
                let first = r.first_choices.get(id).copied().unwrap_or(0);
                let winner = if rank == 0 && r.winner_votes > 0 {
                    let word = if closed { t.msg("Winner") } else { t.msg("Leading") };
                    format!(" <span class=\"bf-badge is-ok\">{}</span>", esc(&word))
                } else {
                    String::new()
                };
                format!(
                    " <span class=\"muted small\">{}</span>{winner}",
                    esc(&t.msgf("First choices: {n}", &[("n", &first.to_string())]))
                )
            } else if show {
                let percent = if total > 0 { votes * 100 / total } else { 0 };
                format!(" <span class=\"muted small\">{votes} · {percent}%</span>")
            } else {
                String::new()
            };
            let you = if *chosen && !can_submit { format!(" {}", esc(&t.msg("(your vote)"))) } else { String::new() };
            let who: Vec<&str> = voters.iter().filter(|v| v.0 == p.id && v.1 == *id).map(|v| v.2.as_str()).collect();
            let who = if show && p.is_public && !who.is_empty() {
                format!(
                    "<details class=\"small\"><summary>{}</summary>{}</details>",
                    esc(&t.n("count.voters", who.len() as i64)),
                    who.iter().map(|n| user_link(Some(n))).collect::<Vec<_>>().join(", ")
                )
            } else {
                String::new()
            };
            html.push_str(&format!("<li><label>{input}{}{you}</label>{count}{who}</li>", esc(label)));
        }
        html.push_str("</ul>");
        if show && p.kind == "rating" && total > 0 {
            let sum: f64 = rows.iter().map(|r| r.2.parse::<f64>().unwrap_or(0.0) * r.3 as f64).sum();
            html.push_str(&format!(
                "<p class=\"small\"><strong>{}</strong></p>",
                esc(&t.msgf("Average: {avg}", &[("avg", &format!("{:.1}", sum / total as f64))]))
            ));
        }
        if let Some(r) = ranked.as_ref().filter(|r| r.counted > 0) {
            html.push_str(&format!(
                "<p class=\"small muted\">{}</p>",
                esc(&t.msgf(
                    "Instant-runoff count, round {round}: {votes} of {counted} ballots for the first place",
                    &[
                        ("round", &r.rounds.to_string()),
                        ("votes", &r.winner_votes.to_string()),
                        ("counted", &r.counted.to_string())
                    ]
                ))
            ));
        }
        if can_submit {
            let label = if voted { t.msg("Change vote") } else { t.msg("Vote") };
            let withdraw = if voted && p.vote_change {
                format!(
                    " <button class=\"link small\" name=\"withdraw\" value=\"1\" formnovalidate>{}</button>",
                    esc(&t.msg("Withdraw vote"))
                )
            } else {
                String::new()
            };
            html.push_str(&format!("<button class=\"btn\">{}</button>{withdraw}</form>", esc(&label)));
        } else if open && !eligible {
            html.push_str(&format!("<p class=\"small muted\">{}</p>", esc(&t.msg("You may not vote in this poll."))));
        }
        let meta: Vec<String> = meta
            .iter()
            .map(|m| match (m.find('\u{e001}'), m.find('\u{e002}')) {
                (Some(a), Some(b)) if a < b => {
                    let time = &m[a + '\u{e001}'.len_utf8()..b];
                    format!("{}{time}{}", esc(&m[..a]), esc(&m[b + '\u{e002}'.len_utf8()..]))
                }
                _ => esc(m),
            })
            .collect();
        html.push_str(&format!("<p class=\"small muted\">{}</p>", meta.join(" · ")));
        if can_manage {
            html.push_str(&manage_controls(&t, item, p, &rows, locked, closed, &csrf_field, &sprite));
        }
        html.push_str("</section>");
    }
    Ok(html)
}

/// This member's rank for each option of a ranked poll.
async fn my_ranks(
    pool: &sqlx::PgPool,
    poll: i64,
    user: i64,
) -> Result<std::collections::HashMap<i64, i16>, sqlx::Error> {
    let rows: Vec<(i64, i16)> = sqlx::query_as(
        "SELECT option_id, rank FROM poll_votes WHERE poll_id = $1 AND user_id = $2 AND rank IS NOT NULL",
    )
    .bind(poll)
    .bind(user)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Edit, close or reopen, and remove: what the author or a moderator can do with a poll.
#[allow(clippy::too_many_arguments)]
fn manage_controls(
    t: &bf_core::i18n::T,
    item: i64,
    p: &Poll,
    rows: &[&(i64, i64, String, i64, bool)],
    locked: bool,
    closed: bool,
    csrf_field: &str,
    sprite: &str,
) -> String {
    let labels: Vec<String> = rows.iter().map(|r| r.2.clone()).collect();
    let id = p.id;
    let mut h = String::from("<div class=\"bf-poll-manage\">");
    h.push_str(&format!(
        "<button type=\"button\" class=\"link small\" data-dialog=\"poll-edit-{id}\">{}</button>\
         <dialog class=\"bf-dialog\" id=\"poll-edit-{id}\" aria-labelledby=\"poll-edit-title-{id}\">\
         <header class=\"bf-dialog-head\"><h2 id=\"poll-edit-title-{id}\">{}</h2><button type=\"button\" class=\"bf-icon-btn\" data-dialog-close aria-label=\"{}\"><svg class=\"bf-icon\" aria-hidden=\"true\"><use href=\"{sprite}#x\"></use></svg></button></header>\
         <form method=\"post\" action=\"/t/{item}/poll/{id}/edit\"><div class=\"bf-dialog-body stack\">{csrf_field}{}</div>\
         <div class=\"bf-dialog-actions\"><button type=\"button\" class=\"btn\" data-dialog-close>{}</button><button class=\"btn primary\">{}</button></div></form></dialog>",
        esc(&t.msg("Edit poll")),
        esc(&t.msg("Edit poll")),
        esc(&t.t("common.close")),
        poll_fields(*t, Some((p, &labels))),
        esc(&t.t("common.cancel")),
        esc(&t.t("common.save")),
    ));
    // Locking the topic closes its polls, and only unlocking opens them again.
    if !locked {
        let (value, label) = if closed { ("0", t.msg("Reopen poll")) } else { ("1", t.msg("Close poll")) };
        h.push_str(&format!(
            "<form method=\"post\" action=\"/t/{item}/poll/{id}/closing\" class=\"inline\">{csrf_field}<input type=\"hidden\" name=\"closed\" value=\"{value}\"><button class=\"link small\">{}</button></form>",
            esc(&label)
        ));
    }
    h.push_str(&format!(
        "<form method=\"post\" action=\"/t/{item}/poll/{id}/delete\" class=\"inline\">{csrf_field}<button class=\"btn bf-btn-danger\">{}</button></form></div>",
        esc(&t.msg("Remove poll"))
    ));
    h
}

use bf_web::ago::Ago;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_time_forms() {
        assert_eq!(parse_when("  ").unwrap(), When::None);
        assert!(matches!(parse_when("2026-12-01T18:30").unwrap(), When::Local(_)));
        // A full timestamp is a definite moment whatever the member's zone.
        let When::At(t) = parse_when("2026-12-01T18:30:00+02:00").unwrap() else { panic!("not a moment") };
        assert_eq!(t.to_rfc3339(), "2026-12-01T16:30:00+00:00");
        assert!(parse_when("tomorrow").is_err());
    }
}
