//! Recurring jobs plugins schedule with `{"op": "schedule"}`: when one is due the host queues a
//! `job.run` for its plugin, with the payload it was scheduled with and `"schedule": name`.

use crate::state::AppState;
use bf_db::platform::plugins;
use serde_json::json;
use std::sync::Once;

use super::listening;

static STARTED: Once = Once::new();

/// Starts the ticker the first time plugins load. It costs no query while no plugin listens
/// for `job.run`.
pub(super) fn ensure_started(state: &AppState) {
    let state = state.clone();
    STARTED.call_once(move || {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(bf_core::scaled(std::time::Duration::from_secs(30)));
            loop {
                tick.tick().await;
                let ids: Vec<String> = listening("job.run").iter().map(|p| p.id.clone()).collect();
                let (Some(pool), false) = (state.pool.clone(), ids.is_empty()) else { continue };
                match plugins::take_due_schedules(&pool, &ids, 50).await {
                    Ok(due) => {
                        for (plugin, name, mut payload) in due {
                            if let Some(map) = payload.as_object_mut() {
                                map.insert("schedule".into(), json!(name));
                            }
                            // A plugin at its job limit simply skips this round.
                            let _ = plugins::enqueue_job(&pool, &plugin, &payload, 0).await;
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "plugin schedules query failed"),
                }
            }
        });
    });
}
