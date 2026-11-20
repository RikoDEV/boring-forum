//! The request context: who is asking (`Viewer`), cookies and CSRF tokens.

use crate::forum::tree::GUESTS;
use crate::moderation::review::review_count;
use crate::state::AppState;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::perms::Actor;
use bf_core::{AppError, auth};
use bf_db::accounts::{sessions, users};
use serde::Deserialize;
use std::net::SocketAddr;

// `__Host-`: the browser refuses these unless Secure, Path=/ and without Domain, so a sibling
// subdomain cannot plant them.
pub(crate) const SESSION_COOKIE: &str = "__Host-bf_session";

/// Holds the admin's own session while they view the site as a member.
pub(crate) const ORIGINAL_SESSION_COOKIE: &str = "__Host-bf_session_admin";

const CSRF_COOKIE: &str = "__Host-bf_csrf";

/// The current visitor: a session user or a guest, plus their CSRF cookie.
#[derive(Clone)]
pub struct Viewer {
    pub user: Option<users::User>,
    pub actor: Actor,
    csrf_cookie: Option<String>,
    pub(crate) session_digest: Option<String>,
    pub ip: Option<std::net::IpAddr>,
    /// Language for this request: member setting, `bf_lang` cookie, Accept-Language, site default.
    pub locale: &'static str,
    /// Set when authenticated by an API key: read, write or admin.
    pub api_scope: Option<String>,
    /// Scopes plugins declared that the API key holds (`plugin:<id>:<key>`).
    pub plugin_scopes: Vec<String>,
    /// An admin is viewing the site as this member.
    pub impersonating: bool,
    /// Pending review items for someone who moderates anything; `None` for everyone else.
    pub(crate) review_count: Option<i64>,
    /// Theme cookies, kept so `layout` can choose a theme without the request.
    pub(crate) theme_headers: HeaderMap,
}

/// Was this request made by a page's `<img>`, `<script>`, `<iframe>` and the like, and not by a
/// person following a link or typing an address? Browsers say so in `Sec-Fetch-Dest`. A GET that
/// changes something (counts a click, sets a cookie) must refuse these: a post can embed any
/// address of this site as a picture, and the reader's browser would load it with their cookies.
/// A client that sends no such header (curl, a feed reader, an old browser) is not one of these.
pub(crate) fn is_subresource(headers: &HeaderMap) -> bool {
    headers.get("sec-fetch-dest").and_then(|h| h.to_str().ok()).is_some_and(|d| d != "document")
}

/// A visitor's language before we know who they are: their cookie, then the language the site is
/// set to for everyone (`default_locale`), or with `browser` their browser's, then English.
pub(crate) fn guest_locale(headers: &HeaderMap, state: &AppState) -> String {
    if let Some(c) = cookie(headers, "bf_lang").filter(|c| bf_core::i18n::supported(c)) {
        return c.to_owned();
    }
    let settings = state.settings();
    let setting = settings.str("default_locale");
    if bf_core::i18n::supported(setting) {
        return setting.to_owned();
    }
    headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|h| h.to_str().ok())
        .and_then(bf_core::i18n::from_accept_language)
        .map_or_else(|| bf_core::i18n::site_language(setting).to_owned(), str::to_owned)
}

impl Viewer {
    /// Messages in this viewer's language.
    pub(crate) fn t(&self) -> bf_core::i18n::T {
        bf_core::i18n::T::new(self.locale)
    }
}

/// The groups an account acts with. A member of a group that requires two-factor who has not set
/// it up keeps an ordinary member's rights until they do: the staff groups are left out, so
/// moderation and the admin panel stay closed (and the settings page, where it is set up, is open).
fn powers_groups(u: &users::User) -> Vec<bf_core::ids::GroupId> {
    if u.needs_2fa {
        u.groups
            .iter()
            .copied()
            .filter(|g| *g != bf_core::ids::GroupId::MODERATORS && *g != bf_core::ids::GroupId::ADMINISTRATORS)
            .collect()
    } else {
        u.groups.clone()
    }
}

/// An account still awaiting approval has exactly a guest's rights: it can log in, see its
/// notice and use its account pages (which need the user, not rights), but reads nothing a guest
/// cannot, in the forums, uploads, search or the live stream. It keeps its id so its own files
/// and account stay its own; `silenced` keeps every write closed.
pub(crate) fn pending_actor(u: &users::User) -> Actor {
    Actor { user_id: Some(u.id), groups: vec![GUESTS.into()], silenced: true, ..Default::default() }
}

/// The rights of a signed-in member's session, without plugin denials (`Actor::denied`): the 2FA
/// demotion of staff groups and moderated forums, and a guest's rights while awaiting approval.
/// Every place that turns a session's user into an actor uses this one.
pub(crate) fn actor_for(u: &users::User) -> Actor {
    if !u.approved {
        return pending_actor(u);
    }
    Actor {
        user_id: Some(u.id),
        groups: powers_groups(u),
        is_admin: u.is_admin && !u.needs_2fa,
        silenced: u.silenced,
        moderated_nodes: if u.needs_2fa { Vec::new() } else { u.moderated_nodes.clone() },
        ..Default::default()
    }
}

pub(crate) fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

impl FromRequestParts<AppState> for Viewer {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
        let ip = peer.map(|p| crate::http::ratelimit::client_ip(p, &parts.headers));
        let csrf_cookie = cookie(&parts.headers, CSRF_COOKIE).map(str::to_owned);
        let locale = bf_core::i18n::T::new(&guest_locale(&parts.headers, state)).locale;
        let mut v = Viewer {
            user: None,
            actor: Actor { groups: vec![GUESTS.into()], ..Default::default() },
            csrf_cookie,
            session_digest: None,
            ip,
            locale,
            api_scope: None,
            plugin_scopes: Vec::new(),
            // Set from the session below: a cookie in the browser proves nothing about it.
            impersonating: false,
            review_count: None,
            theme_headers: parts.headers.get(header::COOKIE).map_or_else(HeaderMap::new, |c| {
                let mut h = HeaderMap::new();
                h.insert(header::COOKIE, c.clone());
                h
            }),
        };
        // API keys: no cookies involved, so no CSRF either; the scope limits what they can do.
        let bearer = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "));
        if let (Some(key), Some(pool)) = (bearer.filter(|b| b.starts_with("bf_")), &state.pool) {
            let (user_id, scope) = bf_db::platform::api::key_owner(pool, &auth::token_digest(key))
                .await
                .map_err(AppError::internal)?
                .ok_or(AppError::Forbidden)?;
            if scope == "read" && !matches!(parts.method, axum::http::Method::GET | axum::http::Method::HEAD) {
                return Err(AppError::Forbidden);
            }
            let mut u = users::user_by_id(pool, user_id.into())
                .await
                .map_err(AppError::internal)?
                .ok_or(AppError::Forbidden)?;
            // Below the admin scope a key is an ordinary member's: nothing may read admin rights
            // off the user or its Administrators group (id 4).
            if scope != "admin" {
                u.is_admin = false;
                u.groups.retain(|g| *g != 4);
            }
            v.actor = Actor {
                user_id: Some(u.id),
                groups: powers_groups(&u),
                is_admin: u.is_admin && scope == "admin" && !u.needs_2fa,
                silenced: u.silenced,
                moderated_nodes: if u.needs_2fa { Vec::new() } else { u.moderated_nodes.clone() },
                denied: crate::platform::plugins::denials(state, u.id.get(), &u.groups).await,
            };
            if !u.approved {
                v.actor = pending_actor(&u);
            }
            v.user = Some(u);
            v.plugin_scopes =
                bf_db::platform::registry_content::key_scopes(pool, &auth::token_digest(key)).await.unwrap_or_default();
            // Resource scopes (`read:topics`, ...) narrow what the key may reach.
            if !crate::platform::api_keys::resource_allowed(&v.plugin_scopes, &parts.method, parts.uri.path()) {
                return Err(AppError::Forbidden);
            }
            v.api_scope = Some(scope);
            return Ok(v);
        }
        let (Some(token), Some(pool)) = (cookie(&parts.headers, SESSION_COOKIE), &state.pool) else {
            return Ok(v);
        };
        let digest = auth::token_digest(token);
        // Cached for seconds and emptied by any write, so changes show at once.
        let found = if let Some(u) = state.sessions.get(&digest) {
            Some(u)
        } else {
            let loaded = sessions::session_user_idle(pool, &digest, state.settings().int("session_idle_minutes"))
                .await
                .map_err(AppError::internal)?;
            if let Some(u) = &loaded {
                let agent = parts.headers.get(header::USER_AGENT).and_then(|h| h.to_str().ok());
                // E.g. BF_GEO_HEADER=CF-IPCountry; no address database is shipped.
                let country = std::env::var("BF_GEO_HEADER").ok().and_then(|name| {
                    let v = parts.headers.get(name.as_str())?.to_str().ok()?;
                    v.chars().all(|c| c.is_ascii_alphanumeric()).then(|| v.to_owned())
                });
                if let Err(e) = bf_db::accounts::sessions::touch_session(pool, &digest, agent, country.as_deref()).await
                {
                    tracing::warn!(error = %e, "session touch failed");
                }
                state.sessions.insert(digest.clone(), u.clone());
            }
            loaded
        };
        if let Some(u) = found {
            v.impersonating = u.impersonated;
            v.actor = actor_for(&u);
            v.actor.denied = crate::platform::plugins::denials(state, u.id.get(), &u.groups).await;
            if let Some(l) = u.locale.as_deref().filter(|l| bf_core::i18n::supported(l)) {
                v.locale = bf_core::i18n::T::new(l).locale;
            }
            v.user = Some(u);
            v.session_digest = Some(digest);
            // Only page views show the badge, so writes and API calls skip the query.
            if parts.method == axum::http::Method::GET && !parts.uri.path().starts_with("/api/") {
                v.review_count = review_count(state, pool, &v.actor).await;
            }
        }
        Ok(v)
    }
}

impl Viewer {
    /// The administrator behind an impersonated session, if this is one.
    pub(crate) fn impersonator(&self) -> Option<i64> {
        if self.impersonating { self.user.as_ref().and_then(|u| u.impersonated_by) } else { None }
    }

    pub(crate) fn require_user(&self) -> Result<&users::User, AppError> {
        self.user.as_ref().ok_or(AppError::Forbidden)
    }

    /// Recently proved who they are. An admin viewing the site as someone never counts.
    pub(crate) fn has_sudo(&self) -> bool {
        !self.impersonating && self.user.as_ref().is_some_and(|u| u.sudo)
    }

    pub(crate) fn require_sudo(&self) -> Result<(), AppError> {
        if self.has_sudo() { Ok(()) } else { Err(AppError::SudoRequired) }
    }

    /// For forms: where to send the browser first, if it has to confirm.
    pub(crate) fn sudo_redirect(&self, back: &str) -> Option<Response> {
        (!self.has_sudo()).then(|| {
            let next: String = url::form_urlencoded::byte_serialize(back.as_bytes()).collect();
            Redirect::to(&format!("/sudo?next={next}")).into_response()
        })
    }

    pub(crate) fn check_csrf(&self, submitted: &str) -> Result<(), AppError> {
        if self.api_scope.is_some() || auth::csrf_matches(self.csrf_cookie.as_deref(), Some(submitted)) {
            Ok(())
        } else {
            Err(AppError::BadRequest("Your session expired. Reload the page and try again.".into()))
        }
    }

    /// `check_csrf` with the token a script sends in the `x-csrf-token` header.
    pub(crate) fn check_csrf_headers(&self, headers: &HeaderMap) -> Result<(), AppError> {
        self.check_csrf(csrf_header(headers))
    }

    /// `check_session_csrf` with the `x-csrf-token` header.
    pub(crate) fn check_session_csrf_headers(&self, headers: &HeaderMap) -> Result<(), AppError> {
        self.check_session_csrf(csrf_header(headers))
    }

    /// For account-security actions: a browser session only. An API key or OAuth token must never
    /// be able to mint longer-lived or stronger credentials than itself.
    pub(crate) fn check_session_csrf(&self, submitted: &str) -> Result<(), AppError> {
        if self.api_scope.is_some() {
            return Err(AppError::Forbidden);
        }
        self.check_csrf(submitted)
    }
}

/// The CSRF token a script sends in the `x-csrf-token` header; empty when there is none.
pub(crate) fn csrf_header(headers: &HeaderMap) -> &str {
    headers.get("x-csrf-token").and_then(|h| h.to_str().ok()).unwrap_or_default()
}

pub(crate) fn csrf_for(v: &Viewer, wanted: bool) -> (String, Option<HeaderValue>) {
    if !wanted {
        return (String::new(), None);
    }
    match &v.csrf_cookie {
        Some(c) if c.len() == 64 => (c.clone(), None),
        _ => {
            let t = auth::random_token();
            let cookie = format!("{CSRF_COOKIE}={t}; Path=/; Secure; SameSite=Lax");
            (t, HeaderValue::from_str(&cookie).ok())
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct CsrfForm {
    pub(crate) csrf: String,
}

/// Staff can see and add notes on members: admins, or anyone who moderates a forum.
/// Site-wide staff: administrators and the moderators group. Moderating one forum, which any
/// club founder does, gives power over that forum only, and none of this.
pub(crate) fn is_staff(v: &Viewer) -> bool {
    v.actor.is_admin || v.actor.groups.contains(&bf_core::ids::GroupId::MODERATORS)
}
