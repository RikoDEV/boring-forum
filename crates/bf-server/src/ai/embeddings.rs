//! Topic embeddings: related topics under a topic, "similar topics" while writing a new one, and
//! search by meaning. One vector per topic (its title and the opening of its first post), kept
//! in the database and searched in memory: a few tens of thousands of topics take milliseconds, and
//! it needs nothing but the Postgres we already run. `ai_embedding_max_topics` caps the memory.

use super::client::{self, clip};
use super::may_send;
use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_core::slug::topic_path;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/topics/{id}/related", get(related)).route("/api/v1/ai/similar", get(similar))
}

struct Row {
    item: i64,
    node: i64,
    vec: Vec<f32>,
}

struct Index {
    model: String,
    built: Instant,
    rows: Vec<Row>,
}

static INDEX: LazyLock<RwLock<Option<Arc<Index>>>> = LazyLock::new(Default::default);
static DIRTY: AtomicBool = AtomicBool::new(false);
static QUERIES: LazyLock<moka::sync::Cache<String, Arc<Vec<f32>>>> =
    LazyLock::new(|| moka::sync::Cache::builder().max_capacity(2000).time_to_live(Duration::from_secs(3600)).build());

pub(crate) fn encode(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub(crate) fn decode(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The topics nearest to `q` among `readable` forums, best first, at least `min` similar.
fn nearest(
    index: &Index,
    q: &[f32],
    readable: &HashSet<i64>,
    k: usize,
    exclude: Option<i64>,
    min: f32,
) -> Vec<(i64, f32)> {
    let mut best: Vec<(i64, f32)> = Vec::with_capacity(k + 1);
    for r in &index.rows {
        if r.vec.len() != q.len() || Some(r.item) == exclude || !readable.contains(&r.node) {
            continue;
        }
        let score = dot(&r.vec, q);
        if score < min || (best.len() == k && best.last().is_some_and(|l| l.1 >= score)) {
            continue;
        }
        let at = best.partition_point(|b| b.1 >= score);
        best.insert(at, (r.item, score));
        best.truncate(k);
    }
    best
}

async fn index(state: &AppState) -> Option<Arc<Index>> {
    let model = state.settings().str("ai_embedding_model").trim().to_owned();
    {
        let held = INDEX.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(i) = held.as_ref()
            && i.model == model
            && !DIRTY.load(Ordering::Relaxed)
            && i.built.elapsed() < Duration::from_secs(600)
        {
            return Some(i.clone());
        }
    }
    let pool = state.pool.as_ref()?;
    let cap = state.settings().int("ai_embedding_max_topics");
    let stored = bf_db::ai::all_embeddings(pool, &model, cap).await.ok()?;
    DIRTY.store(false, Ordering::Relaxed);
    let rows: Vec<Row> =
        stored.into_iter().map(|s| Row { item: s.item_id, node: s.node_id, vec: decode(&s.vec) }).collect();
    let built = Arc::new(Index { model, built: Instant::now(), rows });
    *INDEX.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(built.clone());
    Some(built)
}

/// A search text as a vector; repeated searches cost nothing.
pub(crate) async fn query_vector(state: &AppState, user: Option<i64>, text: &str) -> Result<Arc<Vec<f32>>, AppError> {
    let key = clip(text.trim(), 300).to_lowercase();
    if let Some(v) = QUERIES.get(&key) {
        return Ok(v);
    }
    let mut out = client::embed(state, "search", user, std::slice::from_ref(&key)).await?;
    let v = Arc::new(
        out.pop().ok_or_else(|| AppError::Validation(en("The AI service is not available right now.").into()))?,
    );
    QUERIES.insert(key, v.clone());
    Ok(v)
}

/// Topics whose meaning is close to `text`, among the forums the viewer can read: (topic, score).
pub(crate) async fn by_meaning(
    state: &AppState,
    v: &Viewer,
    text: &str,
    k: usize,
    min: f32,
) -> Result<Vec<(i64, f32)>, AppError> {
    let user = v.user.as_ref().map(|u| u.id.get());
    by_meaning_for(state, &v.actor, user, text, k, min).await
}

/// The same for any reader, such as the bot looking for what the forum already says.
pub(crate) async fn by_meaning_for(
    state: &AppState,
    actor: &bf_core::perms::Actor,
    user: Option<i64>,
    text: &str,
    k: usize,
    min: f32,
) -> Result<Vec<(i64, f32)>, AppError> {
    let q = query_vector(state, user, text).await?;
    let Some(index) = index(state).await else { return Ok(vec![]) };
    let tree = state.forum.get(state.db()?).await?;
    let readable: HashSet<i64> = tree.readable(actor).into_iter().collect();
    Ok(nearest(&index, &q, &readable, k, None, min))
}

/// Cards for the hits the reader may open. The index is a few minutes old, so each topic's forum
/// is checked again as it is now: one moved to a private forum since is left out.
async fn cards(
    state: &AppState,
    actor: &bf_core::perms::Actor,
    hits: &[(i64, f32)],
) -> Result<Vec<serde_json::Value>, AppError> {
    let ids: Vec<i64> = hits.iter().map(|h| h.0).collect();
    let rows = bf_db::ai::topic_cards(state.db()?, &ids).await.db()?;
    let tree = state.forum.get(state.db()?).await?;
    let readable: HashSet<i64> = tree.readable(actor).into_iter().collect();
    Ok(hits
        .iter()
        .filter_map(|(id, score)| {
            let r = rows.iter().find(|r| r.0 == *id && readable.contains(&r.3))?;
            Some(json!({
                "id": r.0,
                "title": r.2,
                "url": topic_path(r.0, &r.1),
                "forum": tree.node(r.3).map(|n| n.title.clone()).unwrap_or_default(),
                "score": (score * 100.0).round() / 100.0,
            }))
        })
        .collect())
}

fn private(mut res: Response, secs: u32) -> Response {
    if let Ok(h) = HeaderValue::from_str(&format!("private, max-age={secs}")) {
        res.headers_mut().insert(header::CACHE_CONTROL, h);
    }
    res
}

/// Topics like this one, for the foot of a topic page.
async fn related(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let (t, tree, _) = readable_topic(&state, &v, id).await?;
    if !client::embeddings_configured(&state) || !state.settings().bool("ai_related_topics_enabled") {
        return Ok(Json(json!([])).into_response());
    }
    let Some(index) = index(&state).await else { return Ok(Json(json!([])).into_response()) };
    let Some(mine) = index.rows.iter().find(|r| r.item == t.id) else { return Ok(Json(json!([])).into_response()) };
    let readable: HashSet<i64> = tree.readable(&v.actor).into_iter().collect();
    let hits = nearest(&index, &mine.vec, &readable, 5, Some(t.id), 0.45);
    Ok(private(Json(cards(&state, &v.actor, &hits).await?).into_response(), 300))
}

#[derive(Deserialize)]
struct SimilarQuery {
    #[serde(default)]
    q: String,
}

/// Topics like a title being typed, so a member can find the answer before asking again.
async fn similar(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<SimilarQuery>,
) -> Result<Response, AppError> {
    v.require_user()?;
    if !client::embeddings_configured(&state) || !state.settings().bool("ai_related_topics_enabled") {
        return Ok(Json(json!([])).into_response());
    }
    let text = q.q.trim();
    if text.chars().count() < 12 {
        return Ok(Json(json!([])).into_response());
    }
    let hits = by_meaning(&state, &v, text, 5, 0.5).await?;
    Ok(private(Json(cards(&state, &v.actor, &hits).await?).into_response(), 60))
}

fn fnv(text: &str) -> String {
    let h = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
}

/// From the tick: topics without a vector get one. A few batches per tick; the backlog of an
/// old forum takes days, which is fine because nothing waits for it.
pub(crate) async fn run_index(state: &AppState) {
    let s = state.settings();
    if !client::embeddings_configured(state)
        || !(s.bool("ai_related_topics_enabled") || s.bool("ai_semantic_search_enabled"))
    {
        return;
    }
    let Some(pool) = &state.pool else { return };
    let model = s.str("ai_embedding_model").trim().to_owned();
    let nodes: Option<Vec<i64>> = if s.bool("ai_public_only") {
        let Ok(tree) = state.forum.get(pool).await else { return };
        let guest = bf_core::perms::Actor { groups: vec![1.into()], ..Default::default() };
        Some(tree.readable(&guest))
    } else {
        None
    };
    for _ in 0..8 {
        let Ok(work) = bf_db::ai::embedding_work(pool, &model, nodes.as_deref(), 32).await else { return };
        if work.is_empty() {
            return;
        }
        let texts: Vec<String> =
            work.iter().map(|(_, _, title, raw)| format!("{title}\n\n{}", clip(raw.trim(), 1500))).collect();
        let vectors = match client::embed(state, "embedding", None, &texts).await {
            Ok(v) => v,
            Err(_) => return,
        };
        for ((item, node, _, _), (vec, text)) in work.iter().zip(vectors.iter().zip(&texts)) {
            if !may_send(state, *node).await {
                continue;
            }
            if let Err(e) =
                bf_db::ai::upsert_embedding(pool, *item, *node, &model, &encode(vec), vec.len() as i32, &fnv(text))
                    .await
            {
                tracing::warn!(error = %e, "embedding could not be saved");
                return;
            }
        }
        DIRTY.store(true, Ordering::Relaxed);
    }
}

/// Throws every vector away, after the model was changed or to start over.
pub(crate) async fn reset(state: &AppState) -> Result<u64, AppError> {
    let n = bf_db::ai::drop_embeddings(state.db()?).await.db()?;
    *INDEX.write().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    #[test]
    fn vectors_survive_storage() {
        let v = vec![0.5, -1.25, 3.0];
        assert_eq!(decode(&encode(&v)), v);
    }

    #[test]
    fn nearest_orders_filters_and_excludes() {
        let rows = vec![
            Row { item: 1, node: 1, vec: unit(&[1.0, 0.0]) },
            Row { item: 2, node: 1, vec: unit(&[1.0, 0.2]) },
            Row { item: 3, node: 3, vec: unit(&[1.0, 0.1]) },
            Row { item: 4, node: 1, vec: unit(&[0.0, 1.0]) },
        ];
        let index = Index { model: "m".into(), built: Instant::now(), rows };
        let q = unit(&[1.0, 0.0]);
        let all: HashSet<i64> = [1, 2].into_iter().collect();
        let hits = nearest(&index, &q, &all, 5, Some(1), 0.5);
        // 1 is excluded, 3 is in a forum the reader cannot see, 4 is unrelated.
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), vec![2]);
        let both: HashSet<i64> = [1, 2].into_iter().collect();
        let top = nearest(&index, &q, &both, 1, None, 0.0);
        assert_eq!(top[0].0, 1);
    }
}
