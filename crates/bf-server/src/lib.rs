//! The web application: routes, middleware and background work, by domain. `app` assembles
//! the router; `state` holds what every request shares.

pub mod account;
pub mod admin;
pub mod ai;
pub mod auth;
pub mod community;
pub mod federation;
pub mod forum;
pub mod http;
pub mod moderation;
pub mod platform;
pub mod state;

use axum::extract::State;
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Router, middleware};
use bf_web::pages::{ErrorPage, Layout, render};
use state::AppState;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

pub fn app(state: AppState) -> Router {
    http::observability::install();
    // Guest pages from every domain that has them, behind the microcache.
    let pages = Router::new()
        .merge(forum::cached_pages())
        .merge(community::cached_pages())
        .layer(middleware::from_fn_with_state(state.microcache.clone(), bf_web::microcache::layer))
        // Outside the cache, so a hit it answers still counts as a visit.
        .layer(middleware::from_fn_with_state(state.clone(), community::browsing::guest_hit));

    Router::new()
        .merge(pages)
        .merge(account::router())
        .merge(admin::router())
        .merge(ai::router())
        .merge(auth::router())
        .merge(community::router())
        .merge(forum::router())
        .merge(http::router())
        .merge(moderation::router())
        .merge(platform::router())
        .route("/assets/{file}", get(bf_web::assets::serve))
        .merge(federation::router())
        .nest("/api/v1", admin::settings::router())
        .nest_service("/admin", admin_spa(&state, &state.config.admin_dir))
        .fallback(not_found)
        // A panic while answering (a decoder choking on a crafted file) is one failed request, not
        // the end of the server. Needs `panic = "unwind"` in the release profile.
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        // Outside the page cache: what a guest is sent gets expiring signatures on its file links.
        .layer(middleware::from_fn_with_state(state.clone(), forum::upload_links::sign_links))
        // Outermost of the two: third-party pictures in what is sent come from imgproxy, when it is on.
        .layer(middleware::from_fn_with_state(state.clone(), forum::imgproxy::layer))
        // Outermost: an address that ended in a 404 and has a permalink is sent on.
        .layer(middleware::from_fn_with_state(state.clone(), admin::permalinks::layer))
        // Innermost, so the matched route template is known.
        .layer(middleware::from_fn_with_state(state.clone(), http::traffic::record))
        .layer(middleware::from_fn(http::observability::track))
        // ActivityPub lookups of profile, forum and topic URLs.
        .layer(middleware::from_fn_with_state(state.clone(), federation::negotiate))
        .layer(middleware::from_fn_with_state(state.clone(), platform::plugins::action_gate))
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::admin_api_gate))
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::login_gate))
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::impersonation_trail))
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::offline_gate))
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::ip_ban_gate))
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::error_logger))
        // Errors a browser navigates into become pages; the API keeps problem+json.
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::html_errors))
        // Idempotent writes and validators on the JSON API. It runs before the gates above
        // (a layer added later wraps the ones before it), so it finds out who is calling itself.
        .layer(middleware::from_fn_with_state(state.clone(), http::api::layer))
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::origin_check))
        // Before everything that reads a body, writes a row or empties a cache: the idempotency
        // layer stores keys and `origin_check` drops cached sessions, and neither may be had
        // without limit by someone who is not even signed in.
        .layer(middleware::from_fn_with_state(state.rate_limiter.clone(), http::ratelimit::layer))
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::security_headers))
        // Outermost of the stateful layers: a plain-HTTP request is turned away before
        // any gate, handler or session cookie read runs.
        .layer(middleware::from_fn_with_state(state.clone(), http::middleware::https_redirect))
        .with_state(state)
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
                .layer(TraceLayer::new_for_http().make_span_with(|req: &axum::extract::Request| {
                    let id = req.headers().get(REQUEST_ID).and_then(|v| v.to_str().ok()).unwrap_or("-");
                    // Reset, magic-link, unsubscribe and OAuth URLs carry a credential in
                    // the query string, so it is redacted before it reaches a log line.
                    let uri = bf_core::log::redact_uri(&req.uri().to_string());
                    let span = tracing::info_span!("req", method = %req.method(), uri = %uri, id = %id);
                    // A caller's W3C trace context makes this request part of its trace.
                    http::observability::adopt_parent(&span, req.headers());
                    span
                }))
                .layer(PropagateRequestIdLayer::new(REQUEST_ID))
                .layer(CompressionLayer::new())
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                ))
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::REFERRER_POLICY,
                    HeaderValue::from_static("strict-origin-when-cross-origin"),
                ))
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::X_FRAME_OPTIONS,
                    HeaderValue::from_static("SAMEORIGIN"),
                )),
        )
}

/// The admin SPA: static files with `index.html` as the client-side routing fallback, served to
/// administrators only (`admin_gate`). The API checks every call as well; this keeps the panel
/// itself, its pages and its code, from anyone else. Never indexed.
fn admin_spa(state: &AppState, dir: &std::path::Path) -> Router {
    let index = ServeFile::new(dir.join("index.html"));
    Router::new()
        .fallback_service(ServeDir::new(dir).fallback(index))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-robots-tag"),
            HeaderValue::from_static("noindex"),
        ))
        .layer(middleware::from_fn_with_state(state.clone(), admin_gate))
}

/// Who may load anything under /admin: a signed-in administrator, in their own session (not while
/// viewing the site as a member, and not with an API key). Everyone else, guests included, gets the
/// ordinary "not found" error page: no redirect to sign in, nothing that confirms a panel is here.
async fn admin_gate(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // An administrator whose required two-factor is not set up has no admin rights yet.
    if !v.user.as_ref().is_some_and(|u| u.is_admin && !u.needs_2fa) || v.impersonating || v.api_scope.is_some() {
        let mut res = bf_core::AppError::NotFound.into_response();
        res.headers_mut().insert(axum::http::header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return res;
    }
    // Content-hashed bundles may stay in the admin's own browser; nothing may sit in a shared cache.
    let immutable = req.uri().path().contains("/_app/immutable/");
    let mut res = next.run(req).await;
    let cache = if immutable { "private, max-age=31536000, immutable" } else { "private, no-store" };
    res.headers_mut().insert(axum::http::header::CACHE_CONTROL, HeaderValue::from_static(cache));
    // The panel's own policy: its inline bootstrap script by hash (`security_headers` leaves a
    // policy that is already set alone).
    if let Ok(csp) = HeaderValue::from_str(&http::middleware::admin_csp(&state.config.admin_dir)) {
        res.headers_mut().insert(axum::http::header::CONTENT_SECURITY_POLICY, csp);
    }
    res
}

/// HTML 404 for pages; API routes return problem+json from their own fallback. Custom page
/// addresses (such as `/about`) are served from here first.
async fn not_found(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
    method: axum::http::Method,
    uri: axum::http::Uri,
) -> Response {
    // The IndexNow key file lives at the root, named after the key.
    if method == axum::http::Method::GET
        && let Some(res) = platform::indexnow::key_file(&state, uri.path())
    {
        return res;
    }
    let t = v.t();
    let (groups, is_admin) = (v.actor.groups.clone(), v.actor.is_admin);
    if method == axum::http::Method::GET
        && let Some(res) = crate::admin::pages::custom_path(&state, v, &uri).await
    {
        return res;
    }
    let mut layout = Layout::new(&state.settings(), Some(&t.msg("Not found")));
    layout.t = t;
    layout.set_nav(&state.settings(), &bf_core::ids::raw(&groups), is_admin);
    let page = ErrorPage { layout, status: 404, message: t.msg("That page does not exist.") };
    (StatusCode::NOT_FOUND, render(&page)).into_response()
}
