//! Domain core: configuration, the error model and the site settings registry.

pub mod auth;
pub mod bounce;
pub mod config;
pub mod diff;
pub mod disposable;
pub mod error;
pub mod html;
pub mod i18n;
pub mod ids;
pub mod log;
pub mod mail;
pub mod nav;
pub mod notifications;
pub mod oauth;
pub mod og;
pub mod perms;
pub mod search;
pub mod secretbox;
pub mod settings;
pub mod slug;
pub mod suggest;
pub mod totp;
pub mod trust;
pub mod watched;
pub mod webauthn;

pub use error::{AppError, AppResult};

/// A server timer, shortened when `BF_TIME_SCALE_PERCENT` is set. Only test runs set it, so
/// background loops and short caches do not make each check wait.
pub fn scaled(d: std::time::Duration) -> std::time::Duration {
    static PERCENT: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| {
        // Ignored on an https site: a production server must keep its real timers.
        if std::env::var("BF_BASE_URL").is_ok_and(|u| u.trim().starts_with("https://")) {
            return 100;
        }
        std::env::var("BF_TIME_SCALE_PERCENT")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|p| (1..=100).contains(p))
            .unwrap_or(100)
    });
    d * *PERCENT / 100
}
