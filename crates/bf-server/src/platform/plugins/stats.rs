//! What the admin page shows about each running plugin (calls, errors, fuel, a short log), and
//! the per-plugin rate limits. In memory: it starts afresh when the server restarts.

use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex, PoisonError};

const LOG_LINES: usize = 200;
const LOG_LINE_CHARS: usize = 500;

#[derive(Default)]
struct Entry {
    calls: u64,
    errors: u64,
    fuel: u64,
    last_error: Option<(String, String)>,
    log: VecDeque<(String, String)>,
}

static STATS: LazyLock<Mutex<HashMap<String, Entry>>> = LazyLock::new(Default::default);

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn with<R>(plugin: &str, f: impl FnOnce(&mut Entry) -> R) -> R {
    let mut all = STATS.lock().unwrap_or_else(PoisonError::into_inner);
    f(all.entry(plugin.to_owned()).or_default())
}

/// A line from `bf.log`, kept in a ring of the last 200 per plugin.
pub(super) fn log(plugin: &str, message: &str) {
    let line: String = message.chars().take(LOG_LINE_CHARS).collect();
    with(plugin, |e| {
        if e.log.len() == LOG_LINES {
            e.log.pop_front();
        }
        e.log.push_back((now(), line));
    });
}

/// Fuel one call used, however it ended.
pub(super) fn record_fuel(plugin: &str, used: u64) {
    with(plugin, |e| e.fuel = e.fuel.saturating_add(used));
}

/// One finished hook call.
pub(super) fn record_call(plugin: &str, error: Option<&str>) {
    with(plugin, |e| {
        e.calls += 1;
        if let Some(msg) = error {
            e.errors += 1;
            e.last_error = Some((now(), msg.chars().take(LOG_LINE_CHARS).collect()));
        }
    });
}

pub(super) fn snapshot(plugin: &str) -> Value {
    with(plugin, |e| {
        json!({
            "calls": e.calls,
            "errors": e.errors,
            "avg_fuel": e.fuel.checked_div(e.calls).unwrap_or(0),
            "last_error": e.last_error.as_ref().map(|(at, message)| json!({ "at": at, "message": message })),
            "log": e.log.iter().rev().map(|(at, message)| json!({ "at": at, "message": message })).collect::<Vec<_>>(),
        })
    })
}

static LIMITS: LazyLock<Mutex<HashMap<(String, &'static str), (i64, i64)>>> = LazyLock::new(Default::default);

/// A fixed-window rate limit per plugin and kind of action: true and counted if `cost` more
/// still fits in `limit` for the current window of `window_secs`.
pub(super) fn allow(plugin: &str, kind: &'static str, window_secs: i64, limit: i64, cost: i64) -> bool {
    let window = chrono::Utc::now().timestamp() / window_secs.max(1);
    let mut all = LIMITS.lock().unwrap_or_else(PoisonError::into_inner);
    let slot = all.entry((plugin.to_owned(), kind)).or_insert((window, 0));
    if slot.0 != window {
        *slot = (window, 0);
    }
    if slot.1.saturating_add(cost) > limit {
        return false;
    }
    slot.1 += cost;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_keeps_the_newest_lines() {
        for i in 0..LOG_LINES + 5 {
            log("ring-test", &format!("line {i}"));
        }
        let s = snapshot("ring-test");
        let lines = s["log"].as_array().unwrap();
        assert_eq!(lines.len(), LOG_LINES);
        assert_eq!(lines[0]["message"], format!("line {}", LOG_LINES + 4));
    }

    #[test]
    fn limits_count_per_plugin_and_kind() {
        assert!(allow("lim-a", "x", 3600, 3, 2));
        assert!(!allow("lim-a", "x", 3600, 3, 2));
        assert!(allow("lim-a", "x", 3600, 3, 1));
        assert!(allow("lim-b", "x", 3600, 3, 3), "another plugin has its own count");
        assert!(allow("lim-a", "y", 3600, 3, 3), "another kind too");
    }

    #[test]
    fn calls_and_errors_are_counted() {
        record_call("count-test", None);
        record_call("count-test", Some("boom"));
        record_fuel("count-test", 100);
        let s = snapshot("count-test");
        assert_eq!((s["calls"].as_u64(), s["errors"].as_u64(), s["avg_fuel"].as_u64()), (Some(2), Some(1), Some(50)));
        assert_eq!(s["last_error"]["message"], "boom");
    }
}
