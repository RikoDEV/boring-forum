//! Calendar events: a time in a time zone, a place, repeats, RSVPs, and iCal feeds.

use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::ui::hidden_csrf;
use crate::http::ui::simple_page;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::community::events::{self, Event};
use chrono::{NaiveDate, Utc};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/t/{id}/event", post(set_event))
        .route("/t/{id}/event/delete", post(remove_event))
        .route("/t/{id}/rsvp", post(rsvp))
        .route("/t/{id}/event.ics", get(event_ics))
        .route("/calendar", get(calendar))
        .route("/calendar.ics", get(calendar_ics))
        .route("/calendar/new", get(calendar_new))
}

#[derive(Deserialize)]
struct EventForm {
    csrf: String,
    date: String,
    #[serde(default)]
    start_time: String,
    #[serde(default)]
    end_time: String,
    #[serde(default)]
    timezone: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    recurrence: String,
    #[serde(default)]
    repeat_until: String,
    #[serde(default)]
    capacity: String,
}

fn hhmm(s: &str) -> Result<Option<&str>, AppError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    chrono::NaiveTime::parse_from_str(s, "%H:%M")
        .map(|_| Some(s))
        .map_err(|_| AppError::Validation("Give times as HH:MM.".into()))
}

/// The event fields as a form sends them (text, possibly empty).
pub(crate) struct EventInput<'a> {
    pub date: &'a str,
    pub start_time: &'a str,
    pub end_time: &'a str,
    pub timezone: &'a str,
    pub location: &'a str,
    pub recurrence: &'a str,
    pub repeat_until: &'a str,
    pub capacity: &'a str,
}

/// An event that passed every check, ready to save.
pub(crate) struct CheckedEvent {
    date: NaiveDate,
    start_time: Option<String>,
    end_time: Option<String>,
    timezone: String,
    location: Option<String>,
    recurrence: &'static str,
    repeat_until: Option<NaiveDate>,
    capacity: Option<i32>,
}

impl CheckedEvent {
    pub(crate) fn as_new(&self) -> events::NewEvent<'_> {
        events::NewEvent {
            date: self.date,
            start_time: self.start_time.as_deref(),
            end_time: self.end_time.as_deref(),
            timezone: &self.timezone,
            location: self.location.as_deref(),
            recurrence: self.recurrence,
            repeat_until: self.repeat_until,
            capacity: self.capacity,
        }
    }
}

/// Checks an event's fields before anything is saved: dates and times, the time zone, the repeat
/// rule, the capacity and the place (through the watched words).
pub(crate) async fn check_event(state: &AppState, v: &Viewer, f: &EventInput<'_>) -> Result<CheckedEvent, AppError> {
    let pool = state.db()?;
    let date = NaiveDate::parse_from_str(f.date.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::Validation("Give a date as YYYY-MM-DD.".into()))?;
    let start_time = hhmm(f.start_time)?;
    let end_time = hhmm(f.end_time)?;
    if end_time.is_some() && start_time.is_none() {
        return Err(AppError::Validation("An end time needs a start time.".into()));
    }
    let timezone = match f.timezone.trim() {
        "" => "UTC",
        tz => tz,
    };
    if !events::timezone_known(pool, timezone).await.db()? {
        return Err(AppError::Validation("Unknown time zone; use a name like Europe/Warsaw.".into()));
    }
    let recurrence = events::RECURRENCES.iter().find(|r| **r == f.recurrence).copied().unwrap_or("none");
    let repeat_until = Some(f.repeat_until.trim())
        .filter(|s| !s.is_empty())
        .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
        .transpose()
        .map_err(|_| AppError::Validation("Give the last date as YYYY-MM-DD.".into()))?;
    if repeat_until.is_some_and(|u| u < date) {
        return Err(AppError::Validation("Repeats must end after the first date.".into()));
    }
    let capacity = Some(f.capacity.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<i32>().ok().filter(|c| *c > 0))
        .map(|c| c.ok_or_else(|| AppError::Validation("A capacity is a whole number of people.".into())))
        .transpose()?;
    // One line of text: no control characters, which would break the calendar feed's lines.
    let location: String = f.location.chars().filter(|c| !c.is_control()).collect();
    let location = match location.trim() {
        "" => None,
        l => Some(crate::moderation::gate::words(state, v, &l.chars().take(200).collect::<String>()).await?.text),
    };
    Ok(CheckedEvent {
        date,
        start_time: start_time.map(str::to_owned),
        end_time: end_time.map(str::to_owned),
        timezone: timezone.to_owned(),
        location,
        recurrence,
        repeat_until,
        capacity,
    })
}

async fn set_event(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<EventForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) && (t.author_id != Some(user.id.get()) || v.actor.silenced) {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let pool = state.db()?;
    let checked = check_event(
        &state,
        &v,
        &EventInput {
            date: &f.date,
            start_time: &f.start_time,
            end_time: &f.end_time,
            timezone: &f.timezone,
            location: &f.location,
            recurrence: &f.recurrence,
            repeat_until: &f.repeat_until,
            capacity: &f.capacity,
        },
    )
    .await?;
    let e = checked.as_new();
    let existed = events::event(pool, id).await.db()?.is_some();
    if !events::set_event(pool, id, &e).await.db()? {
        return Err(AppError::Validation(
            "The capacity cannot be lower than the number of people already going.".into(),
        ));
    }
    if existed {
        // Everyone who answered hears the event changed.
        let going = events::rsvp_users(pool, id).await.db()?;
        crate::community::notifications::event_news(
            &state,
            user.id.get(),
            &going,
            "event_changed",
            t.id,
            &t.slug,
            &t.title,
            &user.username,
        )
        .await;
    }
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}

async fn remove_event(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) && t.author_id != Some(user.id.get()) {
        return Err(AppError::Forbidden);
    }
    let pool = state.db()?;
    let going = events::rsvp_users(pool, id).await.db()?;
    events::remove_event(pool, id).await.db()?;
    crate::community::notifications::event_news(
        &state,
        user.id.get(),
        &going,
        "event_cancelled",
        t.id,
        &t.slug,
        &t.title,
        &user.username,
    )
    .await;
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}

#[derive(Deserialize)]
struct RsvpForm {
    csrf: String,
    /// `going`, `maybe`, `not_going` or `clear`.
    status: String,
}

async fn rsvp(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<RsvpForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    let status = match f.status.as_str() {
        "going" | "maybe" | "not_going" => Some(f.status.as_str()),
        "clear" => None,
        _ => return Err(AppError::BadRequest("unknown RSVP".into())),
    };
    match events::rsvp(state.db()?, id, user.id.get(), status).await.db()? {
        events::Rsvp::Saved => {}
        events::Rsvp::Full => return Err(AppError::Validation("This event is full.".into())),
        events::Rsvp::NoEvent => return Err(AppError::NotFound),
    }
    // The organiser hears about answers that make the event more likely to happen.
    if matches!(status, Some("going" | "maybe"))
        && let Some(organiser) = t.author_id
    {
        crate::community::notifications::event_news(
            &state,
            user.id.get(),
            &[organiser],
            "event_rsvp",
            t.id,
            &t.slug,
            &t.title,
            &user.username,
        )
        .await;
    }
    state.microcache.clear();
    Ok(Redirect::to(&format!("{}#event", topic_path(t.id, &t.slug))).into_response())
}

fn when(e: &Event, t: bf_core::i18n::T) -> String {
    let date = &e.local_start[..10];
    let mut s = if e.all_day {
        date.to_owned()
    } else {
        let end = e
            .local_end
            .as_deref()
            .map(|end| if end[..10] == *date { format!("–{}", &end[11..]) } else { format!(" – {end}") });
        format!("{}{} ({})", e.local_start, end.unwrap_or_default(), e.timezone)
    };
    if e.recurrence != "none" {
        let repeats = match e.recurrence.as_str() {
            "daily" => t.msg("repeats daily"),
            "weekly" => t.msg("repeats weekly"),
            "monthly" => t.msg("repeats monthly"),
            "yearly" => t.msg("repeats yearly"),
            other => other.to_owned(),
        };
        s.push_str(", ");
        match e.repeat_until {
            Some(u) => {
                s.push_str(&t.msgf("{repeats} until {date}", &[("repeats", &repeats), ("date", &u.to_string())]));
            }
            None => s.push_str(&repeats),
        }
    }
    s
}

/// The event block on a topic page, already escaped. Called only for topics that have one.
pub(crate) async fn event_html(
    state: &AppState,
    item: i64,
    viewer: Option<i64>,
    csrf: &str,
    can_manage: bool,
    t: bf_core::i18n::T,
) -> Result<String, AppError> {
    let pool = state.db()?;
    let Some(e) = events::event(pool, item).await.db()? else { return Ok(String::new()) };
    let mine = match viewer {
        Some(u) => events::my_rsvp(pool, item, u).await.db()?,
        None => None,
    };
    let going = events::going(pool, item, 30).await.db()?;
    let csrf = esc(csrf);
    let csrf_field = hidden_csrf(&csrf);
    let mut html = format!(
        "<section class=\"bf-box block event\" id=\"event\"><h2>{}</h2><p><time datetime=\"{}\">{}</time></p>",
        esc(&t.msg("Event")),
        e.starts_at.to_rfc3339(),
        esc(&when(&e, t))
    );
    if let Some(l) = &e.location {
        html.push_str(&format!("<p>{}</p>", esc(l)));
    }
    let full = e.capacity.is_some_and(|c| e.going >= i64::from(c));
    let mut counts =
        t.msgf("{going} going · {maybe} maybe", &[("going", &e.going.to_string()), ("maybe", &e.maybe.to_string())]);
    if let Some(c) = e.capacity {
        counts.push_str(" · ");
        counts.push_str(&if full {
            t.msgf("{n} places, full", &[("n", &c.to_string())])
        } else {
            t.msgf("{n} places", &[("n", &c.to_string())])
        });
    }
    let counts = esc(&counts);
    html.push_str(&format!("<p class=\"small muted\">{counts}</p>"));
    if !going.is_empty() {
        let rows: Vec<String> = going.iter().map(|n| crate::community::people::row(n, "")).collect();
        html.push_str(&format!(
            "<p class=\"small muted\">{}</p>{}",
            esc(&t.msg("Going:")),
            crate::community::people::list(&rows)
        ));
    }
    if viewer.is_some() {
        html.push_str(&format!("<form method=\"post\" action=\"/t/{item}/rsvp\" class=\"bf-manage-row\">{csrf_field}"));
        for (value, label) in [("going", t.msg("Going")), ("maybe", t.msg("Maybe")), ("not_going", t.msg("Not going"))]
        {
            let label = esc(&label);
            let current = mine.as_deref() == Some(value);
            let disabled = value == "going" && full && !current;
            html.push_str(&format!(
                "<button class=\"btn{}\" name=\"status\" value=\"{value}\"{}{}>{label}</button> ",
                if current { " primary" } else { "" },
                if current { " aria-pressed=\"true\"" } else { "" },
                if disabled { " disabled" } else { "" }
            ));
        }
        if mine.is_some() {
            html.push_str(&format!(
                "<button class=\"link small\" name=\"status\" value=\"clear\">{}</button>",
                esc(&t.msg("Clear"))
            ));
        }
        html.push_str("</form>");
    }
    html.push_str(&format!(
        "<p class=\"small\"><a href=\"/t/{item}/event.ics\">{}</a></p>",
        esc(&t.msg("Add to your calendar (.ics)"))
    ));
    if can_manage {
        html.push_str(&format!(
            "<form method=\"post\" action=\"/t/{item}/event/delete\" class=\"inline\">{csrf_field}<button class=\"btn bf-btn-danger\">{}</button></form>",
            esc(&t.msg("Remove event"))
        ));
    }
    html.push_str("</section>");
    Ok(html)
}

#[derive(Deserialize, Default)]
struct CalendarQuery {
    /// `YYYY-MM`; the current month when missing or unreadable.
    #[serde(default)]
    month: String,
}

/// The month grid: Monday first, each day listing its events. On a phone the days without
/// events drop out and the rest read as a list.
async fn calendar(
    State(state): State<AppState>,
    v: Viewer,
    axum::extract::Query(q): axum::extract::Query<CalendarQuery>,
) -> Result<Response, AppError> {
    use chrono::{Datelike, Duration};
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let today = chrono::Utc::now().date_naive();
    let first = NaiveDate::parse_from_str(&format!("{}-01", q.month.trim()), "%Y-%m-%d")
        .ok()
        .filter(|d| (2000..=2100).contains(&d.year()))
        .unwrap_or_else(|| today.with_day(1).unwrap_or(today));
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
    }
    .unwrap_or(first);
    let prev = (first - Duration::days(1)).with_day(1).unwrap_or(first);
    let start = first - Duration::days(i64::from(first.weekday().num_days_from_monday()));
    let last = next - Duration::days(1);
    let end = last + Duration::days(i64::from(6 - last.weekday().num_days_from_monday()));
    let rows = events::events_between(pool, &tree.readable(&v.actor), start, end).await.db()?;
    let t = v.t();
    let month_label =
        |d: NaiveDate| bf_web::ago::Ago::month_long(d.and_hms_opt(12, 0, 0).unwrap_or_default().and_utc()).to_string();
    let mut html = format!(
        "<div class=\"bf-cal-head\"><nav class=\"bf-segments small\" aria-label=\"{}\"><a href=\"/calendar?month={}\">{}</a><a href=\"/calendar\">{}</a><a href=\"/calendar?month={}\">{}</a></nav><h2>{}</h2><a class=\"small\" href=\"/calendar.ics\">{}</a></div>",
        esc(&t.msg("Month")),
        prev.format("%Y-%m"),
        esc(&t.msg("Previous")),
        esc(&t.msg("Today")),
        next.format("%Y-%m"),
        esc(&t.msg("Next")),
        month_label(first),
        esc(&t.msg("Subscribe (.ics)")),
    );
    // Members who may start a topic can add an event: it is a topic with a date, so the button
    // leads to the forum's composer with the calendar part open.
    let targets = if v.user.is_some() && !v.actor.silenced {
        crate::forum::posting::create_targets(&tree, &v.actor)
    } else {
        Vec::new()
    };
    if !targets.is_empty() {
        // Inside the header row, beside the feed link.
        html.truncate(html.len() - "</div>".len());
        let options: String = targets
            .iter()
            .map(|(cat, list)| {
                let opts: String =
                    list.iter().map(|(id, name)| format!("<option value=\"{id}\">{}</option>", esc(name))).collect();
                format!("<optgroup label=\"{}\">{opts}</optgroup>", esc(cat))
            })
            .collect();
        html.push_str(&format!(
            "<button type=\"button\" class=\"btn primary bf-cal-add\" data-dialog=\"new-event\"><svg class=\"bf-icon\" aria-hidden=\"true\"><use href=\"{sprite}#plus\"></use></svg> {add}</button>
             <dialog class=\"bf-dialog\" id=\"new-event\" aria-labelledby=\"new-event-title\">
             <header class=\"bf-dialog-head\"><h2 id=\"new-event-title\">{add}</h2><button type=\"button\" class=\"bf-icon-btn\" data-dialog-close aria-label=\"{close}\"><svg class=\"bf-icon\" aria-hidden=\"true\"><use href=\"{sprite}#x\"></use></svg></button></header>
             <div class=\"bf-dialog-body\"><form method=\"get\" action=\"/calendar/new\" class=\"stack\">
             <p class=\"muted small\">{hint}</p>
             <label class=\"bf-field\"><select name=\"forum\">{options}</select><span class=\"bf-field-label\">{forum}</span></label>
             <label class=\"bf-field\"><input type=\"date\" name=\"date\" value=\"{today}\" required placeholder=\" \"><span class=\"bf-field-label\">{date}</span></label>
             <div class=\"bf-dialog-actions\"><button type=\"button\" class=\"btn\" data-dialog-close>{cancel}</button><button class=\"btn primary\">{next}</button></div></form></div></dialog>",
            sprite = bf_web::assets::ICONS.url(),
            add = crate::http::i18n::trh("Add event"),
            close = crate::http::i18n::trh("Close"),
            hint = crate::http::i18n::trh("An event is a topic with a date. Pick the forum it belongs in; you write the topic next."),
            forum = crate::http::i18n::trh("Forum"),
            date = crate::http::i18n::trh("Date"),
            cancel = crate::http::i18n::trh("Cancel"),
            next = crate::http::i18n::trh("Continue"),
            today = today.format("%Y-%m-%d"),
        ));
        html.push_str("</div>");
    }
    let days = [t.msg("Mon"), t.msg("Tue"), t.msg("Wed"), t.msg("Thu"), t.msg("Fri"), t.msg("Sat"), t.msg("Sun")];
    html.push_str("<table class=\"bf-cal\"><thead><tr>");
    html.extend(days.iter().map(|d| format!("<th scope=\"col\">{}</th>", esc(d))));
    html.push_str("</tr></thead><tbody>");
    let mut day = start;
    while day <= end {
        html.push_str("<tr>");
        for wd in &days {
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
            html.push_str(&format!(
                "<td class=\"{class}\" data-wd=\"{}\"><span class=\"bf-cal-day\">{}</span>",
                esc(wd),
                day.day()
            ));
            if !here.is_empty() {
                html.push_str("<ul>");
                for (e, slug, title) in here {
                    // The time is in the reader's zone once the page has loaded; the hover has the full date.
                    let time = if e.all_day {
                        String::new()
                    } else {
                        format!(
                            "<time datetime=\"{}\" data-format=\"time\" data-tip=\"{}\">{}</time> ",
                            e.starts_at.to_rfc3339(),
                            e.starts_at.format("%Y-%m-%d %H:%M UTC"),
                            esc(&e.local_start[11..16.min(e.local_start.len())])
                        )
                    };
                    html.push_str(&format!(
                        "<li>{time}<a href=\"{}\">{}</a></li>",
                        esc(&topic_path(e.item_id, slug)),
                        esc(title)
                    ));
                }
                html.push_str("</ul>");
            }
            html.push_str("</td>");
            day += Duration::days(1);
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    if rows.is_empty() {
        html.push_str(&crate::community::people::empty("calendar", &t.msg("Nothing coming up.")));
    }
    Ok(simple_page(&state, &v, &v.t().msg("Calendar"), &html, true))
}

#[derive(Deserialize)]
struct NewQuery {
    forum: i64,
    #[serde(default)]
    date: String,
}

/// Where the calendar's Add event button leads: the forum's composer, calendar part open.
async fn calendar_new(
    State(state): State<AppState>,
    v: Viewer,
    axum::extract::Query(q): axum::extract::Query<NewQuery>,
) -> Result<Response, AppError> {
    v.require_user()?;
    let tree = state.forum.get(state.db()?).await?;
    let node = tree.node(q.forum).filter(|n| n.kind == "forum").ok_or(AppError::NotFound)?;
    if !tree.perms(&v.actor, node.id).has(Perms::CREATE) {
        return Err(AppError::NotFound);
    }
    let date = NaiveDate::parse_from_str(&q.date, "%Y-%m-%d").map(|d| format!("&date={d}")).unwrap_or_default();
    Ok(Redirect::to(&format!("{}?event=1{date}#new-topic", bf_core::slug::forum_path(node.id, &node.slug)))
        .into_response())
}

/// RFC 5545 text: backslash, semicolon, comma and newlines escaped.
fn ics_text(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace("\r\n", "\\n").replace(['\n', '\r'], "\\n")
}

/// Folds a content line at 75 octets, on character boundaries (RFC 5545 §3.1).
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut len = 0;
    for c in line.chars() {
        let w = c.len_utf8();
        if len + w > 75 {
            out.push_str("\r\n ");
            len = 1;
        }
        out.push(c);
        len += w;
    }
    out.push_str("\r\n");
    out
}

fn vevent(base: &str, host: &str, e: &Event, slug: &str, title: &str) -> String {
    let url = format!("{base}{}", topic_path(e.item_id, slug));
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let mut lines =
        vec!["BEGIN:VEVENT".to_owned(), format!("UID:event-{}@{host}", e.item_id), format!("DTSTAMP:{stamp}")];
    if e.all_day {
        let d = NaiveDate::parse_from_str(&e.local_start[..10], "%Y-%m-%d").unwrap_or(e.on_date);
        lines.push(format!("DTSTART;VALUE=DATE:{}", d.format("%Y%m%d")));
        lines.push(format!("DTEND;VALUE=DATE:{}", (d + chrono::Duration::days(1)).format("%Y%m%d")));
    } else {
        lines.push(format!("DTSTART:{}", e.starts_at.format("%Y%m%dT%H%M%SZ")));
        if let Some(end) = e.ends_at {
            lines.push(format!("DTEND:{}", end.format("%Y%m%dT%H%M%SZ")));
        }
    }
    if e.recurrence != "none" {
        let mut rule = format!("RRULE:FREQ={}", e.recurrence.to_uppercase());
        if let Some(u) = e.repeat_until {
            rule.push_str(&format!(";UNTIL={}T235959Z", u.format("%Y%m%d")));
        }
        lines.push(rule);
    }
    lines.push(format!("SUMMARY:{}", ics_text(title)));
    if let Some(l) = &e.location {
        lines.push(format!("LOCATION:{}", ics_text(l)));
    }
    lines.push(format!("URL:{url}"));
    lines.push("END:VEVENT".to_owned());
    lines.iter().map(|l| fold(l)).collect()
}

fn calendar_response(state: &AppState, body: &str) -> Response {
    let site = ics_text(state.settings().str("site_name"));
    let ics = format!(
        "{}{}{}{}{body}{}",
        fold("BEGIN:VCALENDAR"),
        fold("VERSION:2.0"),
        fold(&format!("PRODID:-//{site}//Boring Forum//EN")),
        fold(&format!("X-WR-CALNAME:{site}")),
        fold("END:VCALENDAR")
    );
    let mut res = ics.into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/calendar; charset=utf-8"));
    res
}

fn base_url(state: &AppState) -> String {
    state.config.base_url.trim_end_matches('/').to_owned()
}

fn host(state: &AppState) -> String {
    let b = base_url(state);
    b.split_once("://").map_or(b.as_str(), |(_, h)| h).to_owned()
}

async fn event_ics(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    let e = events::event(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
    Ok(calendar_response(&state, &vevent(&base_url(&state), &host(&state), &e, &t.slug, &t.title)))
}

/// Everything this viewer can read; a calendar app subscribing has no session, so it sees the
/// public events.
async fn calendar_ics(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let rows = events::events(pool, &tree.readable(&v.actor)).await.db()?;
    let (base, host) = (base_url(&state), host(&state));
    let body: String = rows.iter().map(|(e, slug, title)| vevent(&base, &host, e, slug, title)).collect();
    Ok(calendar_response(&state, &body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ics_lines_fold_at_75_octets() {
        let line = format!("SUMMARY:{}", "ż".repeat(60));
        let folded = fold(&line);
        assert!(folded.split("\r\n").all(|l| l.len() <= 75));
        assert_eq!(folded.replace("\r\n ", "").trim_end(), line);
        assert_eq!(ics_text("a,b;c\\d\ne"), "a\\,b\\;c\\\\d\\ne");
    }

    #[test]
    fn recurrence_text_goes_through_the_catalog() {
        let e = Event {
            item_id: 1,
            on_date: NaiveDate::from_ymd_opt(2030, 1, 1).unwrap(),
            starts_at: chrono::Utc::now(),
            ends_at: None,
            timezone: "UTC".into(),
            all_day: true,
            location: None,
            recurrence: "weekly".into(),
            repeat_until: NaiveDate::from_ymd_opt(2030, 3, 1),
            capacity: None,
            local_start: "2030-01-01 00:00".into(),
            local_end: None,
            going: 0,
            maybe: 0,
        };
        // No catalog entry for these in the test locale: the English text, placeholders filled.
        assert_eq!(when(&e, bf_core::i18n::T::new("en")), "2030-01-01, repeats weekly until 2030-03-01");
    }
}
