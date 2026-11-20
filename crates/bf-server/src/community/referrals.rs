//! Referrals: `/r/{username}` remembers who sent a visitor for 30 days; if they sign up,
//! they are that member's referral. Members see their link and referrals at `/referrals`, and
//! qualified referrals (trust level 1 reached) can earn groups.

use bf_core::html::esc;
use crate::http::i18n::{trf, trh};
use crate::http::render::DbResult;
use crate::http::ui::{Shell, begin, finish, table_box, when};
use crate::http::viewer::{Viewer, cookie};
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use bf_core::AppError;
use bf_core::slug::profile_path;
use bf_db::community::referrals as db;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/r/{name}", get(link)).route("/referrals", get(page))
}

const COOKIE: &str = "bf_ref";

/// Remembers the referrer and sends the visitor on to the home page.
async fn link(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let mut res = Redirect::to("/").into_response();
    if !state.settings().bool("referrals_enabled") || v.user.is_some() || crate::http::viewer::is_subresource(&headers) {
        return Ok(res);
    }
    if let Some(id) = db::referrer(state.db()?, &name).await.db()? {
        let secure = if state.config.base_url.starts_with("https://") { "; Secure" } else { "" };
        let c = format!("{COOKIE}={id}; Path=/; Max-Age=2592000; HttpOnly; SameSite=Lax{secure}");
        if let Ok(h) = HeaderValue::from_str(&c) {
            res.headers_mut().append(header::SET_COOKIE, h);
        }
    }
    Ok(res)
}

/// After a sign-up: credits the referrer from the cookie, if any, and tells them.
pub(crate) async fn credit(state: &AppState, headers: &HeaderMap, user: i64, username: &str) {
    if !state.settings().bool("referrals_enabled") {
        return;
    }
    let Some(referrer) = cookie(headers, COOKIE).and_then(|c| c.parse::<i64>().ok()) else { return };
    let Some(pool) = &state.pool else { return };
    match db::set_referrer(pool, user, referrer).await {
        Ok(true) => {
            let data = json!({ "by": username, "url": profile_path(user, username) });
            crate::community::notifications::send(state, user, &[referrer], "referral_joined", &data).await;
        }
        Ok(false) => {}
        Err(e) => tracing::warn!(error = %e, "recording a referral failed"),
    }
}

async fn page(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    state.require_feature("referrals_enabled")?;
    let user = v.require_user()?;
    let rows = db::referrals(state.db()?, user.id.get()).await.db()?;
    let ctx = begin(&state, &v);
    let s = state.settings();
    let link = format!("{}/r/{}", state.config.base_url, user.username);
    let qualified = rows.iter().filter(|r| r.3).count();
    let groups = reward_groups(&s);
    let n = s.int("referral_reward_count");
    let reward = if groups.is_empty() || n <= 0 {
        String::new()
    } else {
        let mut names = Vec::new();
        for g in &groups {
            if let Some(name) = bf_db::community::groups::group_name(state.db()?, *g).await.db()? {
                names.push(format!("<strong>{}</strong>", esc(&name)));
            }
        }
        format!(
            "<p>{}</p>",
            trh("Bring {n} members who stay and post, and you join {groups}. So far: {qualified} of {n}.")
                .replace("{n}", &n.to_string())
                .replace("{qualified}", &qualified.to_string())
                .replace("{groups}", &names.join(", "))
        )
    };
    let mut html = format!(
        "<section class=\"bf-box block\"><h2>{tr1}</h2><p>{tr2}</p><p><input readonly value=\"{}\" aria-label=\"{tr3}\" class=\"bf-ref-link\"></p>{reward}</section>",
        esc(&link),
        tr1 = trh("Your link"),
        tr2 = trh("Share this. Anyone who signs up within 30 days of opening it counts as yours."),
        tr3 = trh("Your referral link")
    );
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|(id, name, joined, ok)| {
            vec![
                format!("<a href=\"{}\">{}</a>", esc(&profile_path(*id, name)), esc(name)),
                when(*joined),
                if *ok { trh("Yes") } else { trh("Once they reach trust level 1") },
            ]
        })
        .collect();
    html.push_str(&table_box(
        &trf("Referrals ({n})", &[("n", &rows.len().to_string())]),
        &["Member", "Joined", "Counts"],
        &table_rows,
        "No one yet.",
    ));
    let shell = Shell { title: v.t().msg("Referrals"), noindex: true, ..Shell::default() };
    Ok(finish(&state, &v, ctx, shell, html))
}

/// The reward groups, without the built-in ones (guests, members, moderators, administrators),
/// which no one can earn this way.
fn reward_groups(s: &bf_core::settings::Settings) -> Vec<i64> {
    bf_core::settings::group_ids(s.str("referral_reward_groups")).into_iter().filter(|g| *g > 4).collect()
}

/// From the maintenance tick: hands out the reward groups.
pub(crate) async fn run_tick(state: &AppState) {
    let s = state.settings();
    let (groups, count) = (reward_groups(&s), s.int("referral_reward_count"));
    if !s.bool("referrals_enabled") || groups.is_empty() || count <= 0 {
        return;
    }
    let Some(pool) = &state.pool else { return };
    for group in groups {
        match db::reward(pool, count, group).await {
            Ok(added) => {
                for user in added {
                    crate::http::middleware::forget_sessions_of(state, user);
                    crate::community::notifications::added_to_group(state, user, group).await;
                }
            }
            Err(e) => tracing::warn!(error = %e, group, "referral rewards failed"),
        }
    }
}
