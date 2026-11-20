//! This request's language, for text built deep in the code (page helpers, shared HTML) without
//! passing a translator down: `tr("English")`. The `html_errors` middleware sets it from the
//! viewer for the whole request; outside a request (background jobs, emails) `tr` returns the
//! English, so those pass a `T` for the recipient instead.

use bf_core::i18n::T;
use std::future::Future;

tokio::task_local! {
    static CURRENT: T;
}

/// Runs `fut` with `t` as the language of `tr`.
pub(crate) async fn with_locale<F: Future>(t: T, fut: F) -> F::Output {
    CURRENT.scope(t, fut).await
}

/// This request's translator; English outside a request.
pub(crate) fn lang() -> T {
    CURRENT.try_with(|t| *t).unwrap_or_default()
}

/// Text written in English, in this request's language (`T::msg`). `scripts/i18n.py` finds it.
pub(crate) fn tr(english: &str) -> String {
    lang().msg(english)
}

/// `tr` with `{name}` placeholders filled in.
pub(crate) fn trf(english: &str, vars: &[(&str, &str)]) -> String {
    lang().msgf(english, vars)
}

/// The language a member chose, for mail sent to them; the site's default when unset.
pub(crate) async fn user_t(state: &crate::state::AppState, user_id: i64) -> T {
    let chosen: Option<String> = match &state.pool {
        Some(pool) => sqlx::query_scalar("SELECT locale FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .flatten(),
        None => None,
    };
    T::new(chosen.as_deref().unwrap_or(bf_core::i18n::site_language(state.settings().str("default_locale"))))
}

/// `tr`, escaped for HTML.
pub(crate) fn trh(english: &str) -> String {
    bf_core::html::esc(&tr(english))
}
