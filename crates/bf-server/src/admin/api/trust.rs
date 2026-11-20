//! Trust levels in the panel: the levels and their requirements (add, rename, change, remove the
//! top one), members per level, a preview of what changed requirements would do, and one member's
//! progress.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::community::trust::{describe, keep_requirement, level_label};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::ids::UserId;
use bf_core::trust::{self, Level, Levels, Requirement};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

fn requirement_json(t: &bf_core::i18n::T, r: &Requirement) -> Value {
    json!({
        "label": describe(t, r.kind),
        "current": r.current,
        "needed": r.needed,
        "percent": r.percent(),
        "met": r.met(),
    })
}

/// A level as the editor shows and sends it.
fn level_json(l: &Level) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("level".into(), l.level.into());
    o.insert("name".into(), l.name.clone().into());
    o.insert("auto".into(), l.auto.into());
    for what in trust::SUFFIXES {
        o.insert(what.into(), l.get(what).into());
    }
    Value::Object(o)
}

/// `GET /api/v1/admin/trust`: for each level its name, how many members hold it, how many are
/// nearly at the next one, and its requirements.
pub(super) async fn overview(State(state): State<AppState>, Admin(_, v): AdminRead) -> Result<Response, AppError> {
    let pool = state.db()?;
    let t = v.t();
    let s = state.settings();
    let get = s.trust_value();
    let all = s.trust_levels();
    let top_auto = all.max_auto();
    let counts: HashMap<i16, i64> = bf_db::accounts::users::trust_level_counts(pool).await.db()?.into_iter().collect();
    let population =
        bf_db::accounts::users::trust_population(pool, s.int("trust_recent_days") as i32, top_auto).await.db()?;
    let mut near: HashMap<i16, i64> = HashMap::new();
    for c in &population {
        if c.trust_level < top_auto && trust::nearly_there(&trust::requirements(&get, c.trust_level + 1, &c.metrics()))
        {
            *near.entry(c.trust_level).or_default() += 1;
        }
    }
    let levels: Vec<Value> = all
        .0
        .iter()
        .map(|l| {
            let mut o = level_json(l);
            o["label"] = level_label(&state, &t, l.level).into();
            o["members"] = counts.get(&l.level).copied().unwrap_or(0).into();
            o["near_next"] = near.get(&l.level).copied().unwrap_or(0).into();
            o
        })
        .collect();
    Ok(Json(json!({ "levels": levels, "recent_days": s.int("trust_recent_days"), "limit": trust::LIMIT }))
        .into_response())
}

/// What the editor sends: every level from 0 up, and the window of the recent-activity requirement.
#[derive(Deserialize)]
pub(super) struct LevelsBody {
    levels: Vec<LevelIn>,
    recent_days: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LevelIn {
    name: String,
    auto: bool,
    posts: i64,
    days: i64,
    topics_read: i64,
    reputation: i64,
    likes_received: i64,
    likes_given: i64,
    days_visited: i64,
    recent_posts: i64,
}

/// The levels an editor sent, made consistent (level 0 asks for nothing; a level can only be
/// earned when the one below it is) and checked.
fn build(body: &[LevelIn]) -> Result<Levels, AppError> {
    let mut out: Vec<Level> = Vec::with_capacity(body.len());
    for (i, l) in body.iter().enumerate() {
        let mut level = Level {
            level: i as i16,
            name: l.name.trim().to_owned(),
            auto: false,
            posts: l.posts,
            days: l.days,
            topics_read: l.topics_read,
            reputation: l.reputation,
            likes_received: l.likes_received,
            likes_given: l.likes_given,
            days_visited: l.days_visited,
            recent_posts: l.recent_posts,
        };
        if i == 0 {
            for what in trust::SUFFIXES {
                level.set(what, 0);
            }
        } else if l.auto && out.last().is_some_and(|p| p.level == 0 || p.auto) {
            level.auto = true;
        } else {
            // Given by a moderator: nothing to measure.
            for what in trust::SUFFIXES {
                level.set(what, 0);
            }
        }
        out.push(level);
    }
    let levels = Levels(out);
    Levels::parse(&levels.to_setting())
        .map_err(|_| AppError::Validation("Those trust levels are not valid.".into()))?;
    Ok(levels)
}

/// `PUT /api/v1/admin/trust`: replaces the levels. Removing levels moves the members above the new
/// top down to it, and lowers every feature and forum that asked for a level beyond it.
pub(super) async fn save(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(body): Json<LevelsBody>,
) -> Result<StatusCode, AppError> {
    let levels = build(&body.levels)?;
    let mut next = (*state.settings()).clone();
    let was_top = next.trust_levels().max_level();
    let top = levels.max_level();
    let mut changes: Vec<(&'static str, Value)> = vec![("trust_levels", Value::String(levels.to_setting()))];
    if let Some(days) = body.recent_days {
        changes.push(("trust_recent_days", json!(days)));
    }
    next.set("trust_levels", changes[0].1.clone()).map_err(|e| AppError::Validation(e.to_string()))?;
    if let Some(days) = body.recent_days {
        next.set("trust_recent_days", json!(days)).map_err(|e| AppError::Validation(e.to_string()))?;
    }
    if top < was_top {
        // A feature that needed a level that is gone now needs the new top.
        let gates: Vec<&'static str> = next
            .defs()
            .filter(|d| d.ty == bf_core::settings::SettingType::Int && d.key.ends_with("_trust_level"))
            .map(|d| d.key)
            .collect();
        for key in gates {
            if next.int(key) > i64::from(top) {
                next.set(key, json!(top)).map_err(|e| AppError::Validation(e.to_string()))?;
                changes.push((key, json!(top)));
            }
        }
    }
    if let Some(pool) = &state.pool {
        for (key, value) in &changes {
            bf_db::settings::save(pool, key, value).await.map_err(AppError::internal)?;
            crate::admin::settings::bf_db_audit(pool, who, key, value).await?;
        }
        if top < was_top {
            bf_db::accounts::users::cap_trust_level(pool, top).await.db()?;
        }
    }
    state.replace_settings(next);
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/admin/trust/preview`: how many members would move up or down under the proposed
/// levels, without changing anything.
pub(super) async fn preview(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Json(body): Json<LevelsBody>,
) -> Result<Response, AppError> {
    let levels = build(&body.levels)?;
    let s = state.settings();
    let recent_days = body.recent_days.unwrap_or_else(|| s.int("trust_recent_days")).clamp(1, 3650);
    let get = |k: &str| if k == "trust_recent_days" { recent_days } else { levels.value(k) };
    let top_auto = levels.max_auto();
    let population = bf_db::accounts::users::trust_population(state.db()?, recent_days as i32, top_auto).await.db()?;
    let mut promoted = vec![0_i64; usize::try_from(top_auto).unwrap_or(0)];
    let mut demoted = 0_i64;
    for c in &population {
        let m = c.metrics();
        let level = trust::earned_level(&get, c.trust_level, &m);
        if level > c.trust_level {
            promoted[usize::try_from(level - 1).unwrap_or(0)] += 1;
        } else if (1..=top_auto).contains(&c.trust_level) && !c.staff && !trust::keeps_level(&get, c.trust_level, &m) {
            demoted += 1;
        }
    }
    let above = bf_db::accounts::users::trust_level_counts(state.db()?)
        .await
        .db()?
        .into_iter()
        .filter(|(l, _)| *l > levels.max_level())
        .map(|(_, n)| n)
        .sum::<i64>();
    Ok(Json(json!({
        "evaluated": population.len(),
        "promoted_to": promoted,
        "demoted": demoted,
        "moved_down": above,
    }))
    .into_response())
}

/// `GET /api/v1/admin/users/{id}/trust`: where one member stands.
pub(super) async fn member(
    State(state): State<AppState>,
    Admin(_, v): AdminRead,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let t = v.t();
    let s = state.settings();
    let get = s.trust_value();
    let top_auto = s.trust_levels().max_auto();
    let standing =
        bf_db::accounts::users::trust_standing(state.db()?, UserId::from(id), s.int("trust_recent_days") as i32)
            .await
            .db()?
            .ok_or(AppError::NotFound)?;
    let m = standing.metrics();
    let level = standing.trust_level;
    let next = (level < top_auto).then(|| {
        json!({
            "level": level + 1,
            "name": level_label(&state, &t, level + 1),
            "requirements": trust::requirements(&get, level + 1, &m).iter().map(|r| requirement_json(&t, r)).collect::<Vec<_>>(),
        })
    });
    let keep = (1..=top_auto).contains(&level).then(|| keep_requirement(&get, level, &m)).flatten();
    Ok(Json(json!({
        "level": level,
        "name": level_label(&state, &t, level),
        "locked": standing.locked,
        "paused": standing.paused,
        "next": next,
        "keep": keep.as_ref().map(|r| requirement_json(&t, r)),
    }))
    .into_response())
}
