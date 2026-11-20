//! View counting: unique per member or IP per 8 hours, bots ignored,
//! collected in memory and written in one batched statement every few seconds.

use axum::http::{HeaderMap, header};
use dashmap::DashMap;
use moka::sync::Cache;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

const WINDOW: Duration = Duration::from_secs(8 * 3600);

#[derive(Clone)]
pub struct ViewCounter {
    seen: Cache<(i64, String), ()>,
    pending: Arc<DashMap<i64, i32>>,
}

impl Default for ViewCounter {
    fn default() -> Self {
        Self { seen: Cache::builder().max_capacity(200_000).time_to_live(WINDOW).build(), pending: Arc::default() }
    }
}

fn is_bot(headers: &HeaderMap) -> bool {
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    ua.is_empty()
        || ["bot", "crawl", "spider", "slurp", "curl", "wget", "python", "headless"].iter().any(|b| ua.contains(b))
}

impl ViewCounter {
    /// `who` is `u:{id}` for members, `ip:{addr}` for guests.
    pub fn record_key(&self, item_id: i64, who: String, headers: &HeaderMap) -> bool {
        if is_bot(headers) {
            return false;
        }
        let key = (item_id, who);
        if self.seen.contains_key(&key) {
            return false;
        }
        self.seen.insert(key, ());
        *self.pending.entry(item_id).or_insert(0) += 1;
        true
    }

    pub fn record(&self, item_id: i64, v: &crate::http::viewer::Viewer, headers: &HeaderMap) {
        let who = match (&v.user, v.ip) {
            (Some(u), _) => format!("u:{}", u.id),
            (None, Some(ip)) => format!("ip:{ip}"),
            (None, None) => return,
        };
        self.record_key(item_id, who, headers);
    }

    /// Takes the pending counts; they are lost if the write fails, which is acceptable for views.
    pub fn drain(&self) -> (Vec<i64>, Vec<i32>) {
        let keys: Vec<i64> = self.pending.iter().map(|e| *e.key()).collect();
        let mut ids = Vec::with_capacity(keys.len());
        let mut counts = Vec::with_capacity(keys.len());
        for k in keys {
            if let Some((id, c)) = self.pending.remove(&k) {
                ids.push(id);
                counts.push(c);
            }
        }
        (ids, counts)
    }

    pub async fn flush(&self, pool: &PgPool) {
        let (ids, counts) = self.drain();
        if ids.is_empty() {
            return;
        }
        if let Err(e) = bf_db::forum::reading::add_views(pool, &ids, &counts).await {
            tracing::warn!(error = %e, "view count flush failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::USER_AGENT, "Mozilla/5.0 Firefox/140".parse().unwrap());
        h
    }

    #[test]
    fn counts_unique_viewers_and_skips_bots() {
        let vc = ViewCounter::default();
        assert!(vc.record_key(1, "ip:1.1.1.1".into(), &browser()));
        assert!(!vc.record_key(1, "ip:1.1.1.1".into(), &browser()));
        assert!(vc.record_key(1, "u:7".into(), &browser()));
        assert!(vc.record_key(2, "u:7".into(), &browser()));
        let mut bot = HeaderMap::new();
        bot.insert(header::USER_AGENT, "Googlebot/2.1".parse().unwrap());
        assert!(!vc.record_key(1, "ip:9.9.9.9".into(), &bot));
        let (mut ids, counts) = vc.drain();
        let mut pairs: Vec<_> = ids.drain(..).zip(counts).collect();
        pairs.sort();
        assert_eq!(pairs, [(1, 2), (2, 1)]);
        assert_eq!(vc.drain().0.len(), 0);
    }
}
