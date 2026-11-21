//! Signing in and proving who you are: passwords, registration, recovery, two-factor, sudo mode,
//! magic links, OAuth and OIDC, passkeys, and the forum as an identity provider.

use crate::state::AppState;
use axum::Router;

pub mod altcha;
pub mod breached;
pub mod external;
pub mod identity;
pub mod login;
pub mod magic_link;
pub mod mfa;
pub mod oauth;
pub mod passkeys;
pub mod recovery;
pub mod register;
pub mod sudo;

/// Every route of this part of the site.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(altcha::router())
        .merge(external::router())
        .merge(identity::router())
        .merge(login::router())
        .merge(magic_link::router())
        .merge(mfa::router())
        .merge(oauth::router())
        .merge(passkeys::router())
        .merge(recovery::router())
        .merge(register::router())
        .merge(sudo::router())
}

/// Why a new password is refused under the site's rules, or `None` when it is fine.
pub(crate) fn password_problem(state: &AppState, password: &str, username: &str) -> Option<String> {
    use crate::http::i18n::{tr, trf};
    let s = state.settings();
    let min = s.int("min_password_length").max(8);
    if (password.chars().count() as i64) < min {
        return Some(trf("Password must be at least {n} characters", &[("n", &min.to_string())]));
    }
    let unique = s.int("password_unique_characters");
    if unique > 0 {
        let distinct = password.chars().collect::<std::collections::HashSet<_>>().len() as i64;
        if distinct < unique {
            return Some(trf("Password must contain at least {n} different characters", &[("n", &unique.to_string())]));
        }
    }
    bf_core::auth::validate_password_rest(password, username).err().map(tr)
}

/// Why a username breaks the site's length or reserved-name settings, or `None` when it does not.
pub(crate) fn username_problem(state: &AppState, name: &str) -> Option<String> {
    use crate::http::i18n::{tr, trf};
    let s = state.settings();
    let (min, max) = (s.int("min_username_length"), s.int("max_username_length").max(3));
    let n = name.chars().count() as i64;
    if n < min || n > max {
        return Some(trf(
            "Username must be {min}–{max} characters",
            &[("min", &min.to_string()), ("max", &max.to_string())],
        ));
    }
    let lower = name.to_lowercase();
    let reserved =
        s.str("reserved_usernames").split(',').map(|r| r.trim().to_lowercase()).any(|r| match r.strip_suffix('*') {
            Some(prefix) => !prefix.is_empty() && lower.starts_with(prefix),
            None => !r.is_empty() && r == lower,
        });
    reserved.then(|| tr("That username is reserved"))
}
