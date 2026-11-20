//! Advertisements in pages: the live ads, kept in memory so a page needs no query to
//! show one. The server reloads the list when staff change an ad and on every tick, and takes
//! the impression counts it gathers here into the database.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

pub struct LiveAd {
    pub id: i64,
    pub placement: String,
    /// Cleaned HTML, already wrapped in its link and label.
    pub html: String,
    pub show_groups: Vec<i64>,
    pub hide_groups: Vec<i64>,
    pub every: usize,
    pub weight: u32,
    shown: AtomicU64,
}

impl LiveAd {
    pub fn new(
        id: i64,
        placement: String,
        html: String,
        show: Vec<i64>,
        hide: Vec<i64>,
        every: i32,
        weight: i32,
    ) -> Self {
        Self {
            id,
            placement,
            html,
            show_groups: show,
            hide_groups: hide,
            every: usize::try_from(every.max(1)).unwrap_or(5),
            weight: u32::try_from(weight.max(1)).unwrap_or(1),
            shown: AtomicU64::new(0),
        }
    }

    fn for_groups(&self, groups: &[i64]) -> bool {
        (self.show_groups.is_empty() || self.show_groups.iter().any(|g| groups.contains(g)))
            && !self.hide_groups.iter().any(|g| groups.contains(g))
    }
}

static ADS: LazyLock<RwLock<Arc<Vec<LiveAd>>>> = LazyLock::new(Default::default);
static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

/// Replaces the live list. Call `take_counts` first, or the counts so far are lost.
pub fn set(list: Vec<LiveAd>) {
    *ADS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(list);
}

/// Impressions since the last call, per ad, and resets them.
pub fn take_counts() -> Vec<(i64, i64)> {
    let ads = ADS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    ads.iter()
        .filter_map(|a| {
            let n = a.shown.swap(0, Ordering::Relaxed);
            (n > 0).then(|| (a.id, i64::try_from(n).unwrap_or(i64::MAX)))
        })
        .collect()
}

/// A cheap, lock-free pseudo-random number; ads need variety, not secrecy.
fn next() -> u64 {
    let mut x = SEED.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// One ad for a place, chosen by weight among those this viewer's groups may see.
pub fn pick(placement: &str, groups: &[i64]) -> Option<String> {
    let ads = ADS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let fit: Vec<&LiveAd> = ads.iter().filter(|a| a.placement == placement && a.for_groups(groups)).collect();
    let total: u64 = fit.iter().map(|a| u64::from(a.weight)).sum();
    if total == 0 {
        return None;
    }
    let mut roll = next() % total;
    for a in fit {
        let w = u64::from(a.weight);
        if roll < w {
            a.shown.fetch_add(1, Ordering::Relaxed);
            return Some(a.html.clone());
        }
        roll -= w;
    }
    None
}

/// How often an ad goes between posts for these groups: the smallest `every` among them.
pub fn between_every(groups: &[i64]) -> Option<usize> {
    let ads = ADS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    ads.iter().filter(|a| a.placement == "between_posts" && a.for_groups(groups)).map(|a| a.every).min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_decide_who_sees_an_ad() {
        let a = LiveAd::new(1, "header".into(), "x".into(), vec![], vec![2], 5, 1);
        assert!(a.for_groups(&[1]));
        assert!(!a.for_groups(&[2, 7]));
        let b = LiveAd::new(2, "header".into(), "x".into(), vec![1], vec![], 5, 1);
        assert!(b.for_groups(&[1]));
        assert!(!b.for_groups(&[2]));
    }
}
