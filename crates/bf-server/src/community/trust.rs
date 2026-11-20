//! Trust levels for members: the promotion job, the level's name wherever it is shown, and the
//! page where a member sees what they have, what comes next and what each level unlocks.

use crate::http::render::{DbResult, layout, page};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use bf_core::AppError;
use bf_core::i18n::{T, en};
use bf_core::trust::{self, Kind, Metrics, Requirement};
use bf_web::pages::{TrustLevelRow, TrustPage, TrustRow, render};

pub fn router() -> Router<AppState> {
    Router::new().route("/trust", get(trust_page))
}

/// What a trust level is called: the admin's name for it, or the translated default.
pub(crate) fn level_label(state: &AppState, t: &T, level: i16) -> String {
    if let Some(name) = state.settings().trust_levels().custom_name(level) {
        return name.to_owned();
    }
    match trust::default_name(level) {
        Some(name) => t.msg(name),
        None => t.msgf("Level {n}", &[("n", &level.to_string())]),
    }
}

/// The refusal when a feature needs a higher level than the member has: it names both levels
/// and says where to follow their progress.
pub(crate) fn level_refusal(state: &AppState, t: &T, have: i16, need: i64) -> AppError {
    let need = state.settings().trust_levels().clamp(need);
    AppError::invalid(
        "This needs trust level {need} ({need_name}). You are at level {have} ({have_name}). Follow your progress at /trust.",
        &[
            ("need", &need.to_string()),
            ("need_name", &level_label(state, t, need)),
            ("have", &have.to_string()),
            ("have_name", &level_label(state, t, have)),
        ],
    )
}

/// Why a control is disabled for a member below the level a feature needs, or `None` when they
/// may use it (staff always may).
pub(crate) fn needs_text(state: &AppState, t: &T, v: &Viewer, setting: &str) -> Option<String> {
    let user = v.user.as_ref()?;
    let need = state.settings().trust_levels().clamp(state.settings().int(setting));
    if user.is_admin || crate::http::viewer::is_staff(v) || user.trust_level >= need {
        return None;
    }
    Some(t.msgf(
        "Needs trust level {need} ({name}). You are at level {have}.",
        &[("need", &need.to_string()), ("name", &level_label(state, t, need)), ("have", &user.trust_level.to_string())],
    ))
}

/// Settings that gate a feature by trust level, with what the feature is. Level N lists the
/// gates set to N, so the table of levels is always what the settings really say.
const GATES: &[(&str, &str)] = &[
    ("chat_min_trust_level", en("Write in chat channels")),
    ("chat_dm_min_trust_level", en("Start a direct message in chat")),
    ("chat_create_min_trust_level", en("Start an invite-only chat channel")),
    ("pm_min_trust_level", en("Send private messages")),
    ("username_change_min_trust_level", en("Change your username")),
    ("wiki_min_trust_level", en("Edit wiki posts")),
    ("club_min_trust_level", en("Start a club")),
];

/// The features that open at `level`, in the viewer's language.
pub(crate) fn unlocks(state: &AppState, t: &T, level: i16) -> Vec<String> {
    let s = state.settings();
    GATES
        .iter()
        .filter(|(key, _)| s.int(key) == i64::from(level))
        .map(|(_, what)| t.msg(what))
        .collect()
}

/// What a requirement measures, in words.
pub(crate) fn describe(t: &T, kind: Kind) -> String {
    match kind {
        Kind::Posts => t.msg("Posts written"),
        Kind::Days => t.msg("Days since joining"),
        Kind::TopicsRead => t.msg("Topics read"),
        Kind::Reputation => t.msg("Reputation"),
        Kind::LikesReceived => t.msg("Likes received"),
        Kind::LikesGiven => t.msg("Likes given"),
        Kind::DaysVisited => t.msg("Days visited"),
        Kind::Recent { days } => t.msgf("Posts in the last {days} days", &[("days", &days.to_string())]),
    }
}

fn rows(t: &T, reqs: &[Requirement]) -> Vec<TrustRow> {
    reqs.iter()
        .map(|r| TrustRow {
            label: describe(t, r.kind),
            current: r.current.min(r.needed),
            needed: r.needed,
            percent: r.percent(),
            met: r.met(),
        })
        .collect()
}

/// What `level` needs to be kept, with the member's progress; `None` when the site does not ask.
pub(crate) fn keep_requirement(value: &dyn Fn(&str) -> i64, level: i16, m: &Metrics) -> Option<Requirement> {
    let needed = value(&format!("trust_level_{level}_recent_posts"));
    (value(&format!("trust_level_{level}_auto")) == 1 && needed > 0).then(|| Requirement {
        kind: Kind::Recent { days: value("trust_recent_days") },
        needed,
        current: m.recent_posts,
    })
}

/// Raises members whose activity earns a higher level, and tells those who are nearly there.
/// Runs from the maintenance tick, and reads the same requirements the member's page shows.
pub(crate) async fn run_promotion(state: &AppState) {
    promote(state, true).await;
}

/// `run_promotion`, with or without telling the members. An import raises levels without telling:
/// those who are close are marked as told, so the next run does not announce it either.
pub(crate) async fn promote(state: &AppState, announce: bool) {
    let Some(pool) = &state.pool else { return };
    let s = state.settings();
    let get = s.trust_value();
    let levels = s.trust_levels();
    // Cheap numbers at the "nearly there" share of each requirement find everyone who is ready
    // and everyone who is close, in one pass.
    let share = |v: i64| (v * trust::NEARLY_PERCENT + 99) / 100;
    let n = |what: &str| levels.needs(what);
    let candidates = match bf_db::accounts::users::trust_candidates(
        pool,
        n("posts").into_iter().map(share).collect(),
        n("days").into_iter().map(|d| share(d) as i32).collect(),
        n("reputation").into_iter().map(share).collect(),
        s.int("trust_recent_days") as i32,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => return tracing::warn!(error = %e, "trust promotion failed"),
    };
    let mut raises = Vec::new();
    let mut nudges = Vec::new();
    for c in &candidates {
        let m = c.metrics();
        let level = trust::earned_level(&get, c.trust_level, &m);
        if level > c.trust_level {
            raises.push((c.id, level));
        } else {
            let next = c.trust_level + 1;
            if c.nudged_level < next && trust::nearly_there(&trust::requirements(&get, next, &m)) {
                nudges.push((c.id, next));
            }
        }
    }
    match bf_db::accounts::users::raise_trust(pool, &raises).await {
        Ok(raised) => {
            for (user, level) in raised {
                if announce {
                    crate::community::notifications::trust_raised(state, user, level).await;
                    crate::platform::webhooks::fire(state, "trust.changed", serde_json::json!({ "user_id": user, "to": level, "by": "auto" }));
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "trust promotion failed"),
    }
    if !nudges.is_empty() {
        if announce {
            for (user, level) in &nudges {
                crate::community::notifications::trust_near(state, *user, *level).await;
            }
        }
        if let Err(e) = bf_db::accounts::users::mark_trust_nudged(pool, &nudges).await {
            tracing::warn!(error = %e, "trust nudge bookkeeping failed");
        }
    }
}

async fn trust_page(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let t = v.t();
    let s = state.settings();
    let get = s.trust_value();
    let (top, top_auto) = (s.trust_levels().max_level(), s.trust_levels().max_auto());
    let standing =
        bf_db::accounts::users::trust_standing(state.db()?, user.id, s.int("trust_recent_days") as i32)
            .await
            .db()?
            .ok_or(AppError::NotFound)?;
    let level = standing.trust_level;
    let m = standing.metrics();
    let (l, set) = layout(&state, &v, Some(&t.msg("Your trust level")));

    let mut note = None;
    let mut next = None;
    let mut progress = Vec::new();
    let mut keep = Vec::new();
    let mut ready = false;
    if level < top_auto {
        let reqs = trust::requirements(&get, level + 1, &m);
        ready = trust::all_met(&reqs);
        next = Some((level + 1, level_label(&state, &t, level + 1)));
        progress = rows(&t, &reqs);
    }
    if (1..=top_auto).contains(&level) {
        // Kept only while the recent-activity window is met.
        let recent: Vec<Requirement> = keep_requirement(&get, level, &m).into_iter().collect();
        keep = rows(&t, &recent);
    }
    if standing.locked {
        note = Some(t.msg("A moderator set your level, so it does not change by itself."));
    } else if standing.paused && level < top_auto {
        note = Some(t.msg("You are not promoted while your account is suspended or silenced."));
    } else if level >= top {
        note = Some(t.msg("This is the highest level."));
    } else if level == top_auto && keep.is_empty() {
        note = Some(t.msg("This is the highest level you can earn."));
    } else if ready {
        note = Some(t.msg("You meet every requirement. Your level goes up at the next check, within ten minutes."));
    }

    let levels = (0..=top)
        .map(|n| TrustLevelRow {
            level: n,
            name: level_label(&state, &t, n),
            is_current: n == level,
            needs: if n == 0 {
                Vec::new()
            } else if n <= top_auto {
                trust::requirements(&get, n, &Metrics::default())
                    .iter()
                    .map(|r| format!("{}: {}", describe(&t, r.kind), r.needed))
                    .collect()
            } else {
                vec![t.msg("Given by a moderator")]
            },
            unlocks: unlocks(&state, &t, n),
        })
        .collect();

    let mut limits = Vec::new();
    if level == 0 {
        let n = |k: &str| s.int(k);
        let posts = n("new_user_posts");
        if posts > 0 && n("new_user_max_links") >= 0 {
            limits.push(t.msgf(
                "Your first {posts} posts can have at most {links} links.",
                &[("posts", &posts.to_string()), ("links", &n("new_user_max_links").to_string())],
            ));
        }
        if n("approve_first_posts") > 0 {
            limits.push(t.msgf(
                "A moderator checks your first {n} posts before they appear.",
                &[("n", &n("approve_first_posts").to_string())],
            ));
        }
        if n("max_topics_in_first_day") > 0 {
            limits.push(t.msgf(
                "In your first day you can start {n} topics.",
                &[("n", &n("max_topics_in_first_day").to_string())],
            ));
        }
        if n("max_replies_in_first_day") > 0 {
            limits.push(t.msgf(
                "In your first day you can post {n} replies.",
                &[("n", &n("max_replies_in_first_day").to_string())],
            ));
        }
        if n("flood_wait_seconds") > 0 {
            limits.push(t.msgf("You can post once every {n} seconds.", &[("n", &n("flood_wait_seconds").to_string())]));
        }
    }

    let p = TrustPage {
        layout: l,
        level,
        name: level_label(&state, &t, level),
        note,
        next,
        progress,
        keep,
        at_risk: (1..=top_auto).contains(&level) && !standing.locked && !trust::keeps_level(&get, level, &m),
        levels,
        limits,
    };
    Ok(page(render(&p), set, false))
}
