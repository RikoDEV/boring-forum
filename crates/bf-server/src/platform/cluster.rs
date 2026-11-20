//! Several nodes behind one load balancer (`BF_CLUSTER=1`): realtime messages, microcache
//! clears and announcement reloads cross over Postgres `LISTEN/NOTIFY`. With one node
//! nothing here runs: no extra connection, no extra query.

use crate::state::AppState;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;

const CHANNEL: &str = "bf_cluster";
/// Postgres limits a NOTIFY payload to 8000 bytes.
const MAX_PAYLOAD: usize = 7500;

pub fn enabled() -> bool {
    std::env::var("BF_CLUSTER").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        && !std::env::var("BF_DEVDB").is_ok_and(|v| v == "1")
}

/// A name for this process, so a node ignores its own notifications.
fn node_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

/// Wires the hub and the microcache to Postgres and starts the listener.
pub fn spawn(state: &AppState) {
    let Some(pool) = state.pool.clone() else { return };
    if !enabled() {
        return;
    }
    let node = node_id();
    tracing::info!(%node, "cluster mode: realtime and cache invalidation cross nodes over LISTEN/NOTIFY");
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let t = tx.clone();
    let n = node.clone();
    state.hub.set_bridge(move |m| {
        let mut p = json!({ "node": n, "kind": "sse", "channel": m.channel, "event": m.event, "data": m.data }).to_string();
        if p.len() > MAX_PAYLOAD {
            // Too big for NOTIFY: the other nodes' viewers reload what they show.
            p = json!({ "node": n, "kind": "sse", "channel": m.channel, "event": bf_realtime::RESYNC, "data": Value::Null })
                .to_string();
        }
        let _ = t.send(p);
    });
    let t = tx.clone();
    let n = node.clone();
    state.microcache.set_on_clear(move || {
        let _ = t.send(json!({ "node": n, "kind": "microcache" }).to_string());
    });
    ANNOUNCE.get_or_init(|| (tx, node.clone()));

    let send_pool = pool.clone();
    tokio::spawn(async move {
        while let Some(p) = rx.recv().await {
            if let Err(e) = sqlx::query("SELECT pg_notify('bf_cluster', $1)").bind(p).execute(&send_pool).await {
                tracing::warn!(error = %e, "cluster notify failed");
            }
        }
    });

    let state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(e) = listen(&pool, &state, &node).await {
                tracing::warn!(error = %e, "cluster listener stopped; retrying");
            }
            tokio::time::sleep(bf_core::scaled(Duration::from_secs(5))).await;
        }
    });
}

static ANNOUNCE: std::sync::OnceLock<(mpsc::UnboundedSender<String>, String)> = std::sync::OnceLock::new();

/// Tells the other nodes that `kind` changed (`announcements`); a no-op on a single node.
pub fn notify(kind: &str) {
    if let Some((tx, node)) = ANNOUNCE.get() {
        let _ = tx.send(json!({ "node": node, "kind": kind }).to_string());
    }
}

async fn listen(pool: &sqlx::PgPool, state: &AppState, node: &str) -> Result<(), sqlx::Error> {
    let mut l = sqlx::postgres::PgListener::connect_with(pool).await?;
    l.listen(CHANNEL).await?;
    // Whatever happened while this listener was down: viewers reload, pages are rebuilt.
    state.microcache.clear_local();
    loop {
        let n = l.recv().await?;
        let Ok(v) = serde_json::from_str::<Value>(n.payload()) else { continue };
        if v["node"].as_str() == Some(node) {
            continue;
        }
        match v["kind"].as_str() {
            Some("sse") => {
                if let (Some(c), Some(e)) = (v["channel"].as_str(), v["event"].as_str()) {
                    state.hub.publish_remote(c, e, v["data"].clone());
                }
            }
            Some("microcache") => state.microcache.clear_local(),
            Some("announcements") => {
                if let Err(e) = crate::admin::announcements::load_announcements_local(state).await {
                    tracing::warn!(error = ?e, "announcements not reloaded");
                }
            }
            _ => {}
        }
    }
}
