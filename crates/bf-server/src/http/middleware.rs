//! Middleware: security headers, the HTTPS redirect, origin checks, error logging and the
//! offline, login and IP-ban gates; and the CSP report endpoint.

use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use bf_core::AppError;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/csp-report", post(csp_report))
}

/// Pages that belong to one member, to staff, or to a flow (sign-in, settings, mail links, forms),
/// by their first path segment. They get `X-Robots-Tag: noindex` as a second wall behind the
/// `<meta name="robots">` the page builder adds.
fn is_utility_path(path: &str) -> bool {
    const PRIVATE: &[&str] = &[
        "settings",
        "messages",
        "notifications",
        "modcp",
        "review",
        "staff",
        "login",
        "logout",
        "register",
        "forgot",
        "reset",
        "auth",
        "oauth",
        "sudo",
        "verify",
        "confirm-email",
        "welcome",
        "referrals",
        "unsubscribe",
        "translate",
        "scheduled",
        "assigned",
        "read-all",
        "unread",
        "safe-mode",
        "impersonate",
        "newsletter",
        "contact",
        "language",
        "theme",
        "embed",
        "chat",
        "search",
        "api",
        "plugin-slot",
        "status-reply",
        "following",
        "statements",
    ];
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    PRIVATE.contains(&first)
}

/// One more origin the forms of a page may end up at, put in the response's extensions by its
/// handler: the policy allows forms to post to this site only, and a browser applies that to the
/// redirect answering a form too (the consent page of `/oauth/authorize` returns to the application).
#[derive(Clone)]
pub(crate) struct FormActionOrigin(pub String);

/// Every `.html` file of the built admin panel, without its `_app` bundles.
fn admin_html_files(dir: &std::path::Path, depth: u8, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            if depth < 4 && e.file_name() != "_app" {
                admin_html_files(&path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|x| x == "html") {
            out.push(path);
        }
    }
}

/// The text of each inline `<script>` in a page, exactly as the browser hashes it.
fn inline_scripts(html: &str) -> Vec<&str> {
    // ASCII lowering keeps every byte offset.
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(open) = lower[at..].find("<script").map(|i| at + i) {
        let Some(body) = lower[open..].find('>').map(|i| open + i + 1) else { break };
        let Some(end) = lower[body..].find("</script").map(|i| body + i) else { break };
        if !lower[open..body].contains("src=") {
            out.push(&html[body..end]);
        }
        at = end;
    }
    out
}

/// The admin panel's policy. The panel is a static SvelteKit build whose pages start with
/// an inline bootstrap script, and it runs the site's island loader for the header it shares with
/// the site: both are allowed by hash, like the inline code of the site's own pages. The hashes
/// are read from the built HTML itself, again whenever the build changes, so they cannot drift
/// from what is served.
pub(crate) fn admin_csp(dir: &std::path::Path) -> String {
    type Built = (std::path::PathBuf, Option<std::time::SystemTime>, String);
    static CACHE: std::sync::Mutex<Option<Built>> = std::sync::Mutex::new(None);
    let stamp = std::fs::metadata(dir.join("index.html")).and_then(|m| m.modified()).ok();
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((d, s, csp)) = cache.as_ref()
        && d == dir
        && *s == stamp
    {
        return csp.clone();
    }
    let mut hashes = vec![bf_web::security::hash(&bf_web::security::loader_script())];
    let mut files = Vec::new();
    admin_html_files(dir, 0, &mut files);
    for file in files {
        let Ok(html) = std::fs::read_to_string(&file) else { continue };
        for script in inline_scripts(&html) {
            // The browser's parser turns CRLF into LF before it hashes an inline element.
            let hash = bf_web::security::hash(&script.replace("\r\n", "\n"));
            if !hashes.contains(&hash) {
                hashes.push(hash);
            }
        }
    }
    let csp = format!(
        "default-src 'self'; script-src 'self' {}; style-src 'self' 'unsafe-inline'; img-src 'self' https: data:; object-src 'none'; base-uri 'none'; frame-ancestors 'self'; form-action 'self'; report-uri /api/v1/csp-report; report-to csp",
        hashes.join(" ")
    );
    *cache = Some((dir.to_owned(), stamp, csp.clone()));
    csp
}

/// Security headers. The admin SPA is a static SvelteKit build with inline bootstrap
/// code, so `admin_gate` gives what it serves its own policy (`admin_csp`); uploads already set
/// a stricter one. Everything else, the error page a stranger gets under /admin included, gets
/// the site's policy.
pub async fn security_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    use axum::http::{HeaderValue, header};
    let register = req.uri().path() == "/register";
    let private_path = is_utility_path(req.uri().path());
    // Pages leave the custom code out in an admin's safe mode; so does their policy.
    let safe_mode = crate::admin::themes::safe_mode_cookie(req.headers());
    let mut res = next.run(req).await;
    let form_origin = res.extensions().get::<FormActionOrigin>().map(|o| o.0.clone());
    let h = res.headers_mut();
    let mut csp = {
        let s = state.settings();
        // Only the sign-up form loads a challenge, so only it opens up to the provider.
        let provider = if register { s.str("captcha_provider") } else { "" };
        let analytics = bf_web::security::Analytics::from_settings(&s);
        let custom = bf_web::custom_code::from_settings(&s);
        let custom = Some(&*custom).filter(|c| !safe_mode && !c.is_empty());
        let gifs =
            s.bool("gifs_enabled").then(|| s.str("gif_provider")).filter(|_| !s.str("gif_api_key").trim().is_empty());
        let imgproxy = crate::forum::imgproxy::Config::from_settings(&s, &state.config.base_url).map(|c| c.origin());
        bf_web::security::content_security_policy(
            provider,
            s.str("csp_image_hosts"),
            &analytics,
            custom,
            gifs,
            imgproxy.as_deref(),
        )
    };
    if let Some(origin) = form_origin {
        csp = csp.replacen("form-action 'self'", &format!("form-action 'self' {origin}"), 1);
    }
    if !h.contains_key(header::CONTENT_SECURITY_POLICY)
        && let Ok(v) = HeaderValue::from_str(&csp)
    {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    // Private and utility pages stay out of search results whatever their handler remembered to say.
    if private_path && !h.contains_key("x-robots-tag") {
        h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    }
    h.insert("permissions-policy", HeaderValue::from_static("camera=(), microphone=(), geolocation=()"));
    // The Reporting API's endpoint for `report-to csp`; `report-uri` stays for older browsers.
    h.insert("reporting-endpoints", HeaderValue::from_static("csp=\"/api/v1/csp-report\""));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    if state.config.base_url.starts_with("https://") {
        let hsts = if state.config.hsts_preload {
            "max-age=31536000; includeSubDomains; preload"
        } else {
            "max-age=31536000; includeSubDomains"
        };
        h.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static(hsts));
    }
    res
}

/// Answers a plain-HTTP request with a permanent redirect to the HTTPS URL, before any
/// handler runs, so a cookie is never read from or set on an unencrypted request. Runs ahead of
/// the HSTS header above, which only a browser that has already been here once will honour.
pub async fn https_redirect(State(state): State<AppState>, req: Request, next: Next) -> Response {
    use axum::http::{HeaderValue, StatusCode, header};
    use bf_core::config::HttpsRedirect;

    let mode = state.config.https_redirect;
    // Probes and the scraper talk to the instance directly, often over plain HTTP.
    let probe = matches!(req.uri().path(), "/livez" | "/readyz" | "/metrics");
    if mode == HttpsRedirect::Off || probe {
        return next.run(req).await;
    }
    // Same rule as `X-Forwarded-For`: the header counts only from a peer in BF_TRUSTED_PROXIES.
    // Without ConnectInfo (unit tests) the peer is unknown and therefore not trusted.
    let trusted = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .is_some_and(|c| crate::http::ratelimit::is_trusted_proxy(c.0.ip()));
    let header = req.headers().get("x-forwarded-proto").and_then(|v| v.to_str().ok());
    if request_is_secure(mode, state.config.tls_cert.is_some(), trusted, header) {
        return next.run(req).await;
    }
    let path = req.uri().path_and_query().map_or("/", |p| p.as_str());
    let target = format!("{}{path}", state.config.base_url);
    match HeaderValue::try_from(target) {
        // 308 keeps the method and body, so a POST is not silently turned into a GET.
        Ok(location) => (StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, location)]).into_response(),
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

/// Whether a request counts as secure. `X-Forwarded-Proto` is read only when `peer_trusted`.
fn request_is_secure(
    mode: bf_core::config::HttpsRedirect,
    terminates_tls: bool,
    peer_trusted: bool,
    header: Option<&str>,
) -> bool {
    use bf_core::config::HttpsRedirect;
    let forwarded = header.filter(|_| peer_trusted).map(str::to_ascii_lowercase);
    // The first value is the one the outermost proxy saw.
    match forwarded.as_deref().and_then(|v| v.split(',').next()).map(str::trim) {
        Some("https") => true,
        Some(_) => false,
        // No (believed) protocol header. Under `proxied` that is a proxy we cannot read, and
        // redirecting would risk a loop, so the request is let through. Under `always` it is a
        // direct plain connection -- unless this process terminates TLS, in which case the only
        // way to reach this router is over TLS and the plain port is served by the redirector
        // in `main`.
        None => mode == HttpsRedirect::Proxied || terminates_tls,
    }
}

/// Refuses cross-site writes, on top of SameSite cookies and CSRF tokens. Machine
/// callers and Apple's cross-site form post authenticate differently and are exempt.
pub async fn origin_check(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let write = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let path = req.uri().path();
    let exempt = path.starts_with("/api/v1/webhooks/")
        || path == "/api/v1/csp-report"
        // Only the providers' callbacks (Apple posts its answer from another site), nothing else under /auth/.
        || (path.starts_with("/auth/") && path.ends_with("/callback") && path.matches('/').count() >= 3)
        || path == "/oauth/token";
    if write && !exempt {
        let h = req.headers();
        let site = h.get("sec-fetch-site").and_then(|v| v.to_str().ok());
        let origin = h.get(axum::http::header::ORIGIN).and_then(|v| v.to_str().ok());
        let host = h.get(axum::http::header::HOST).and_then(|v| v.to_str().ok()).unwrap_or_default();
        let same = |o: &str| {
            o == state.config.base_url.trim_end_matches('/')
                || o.split_once("://").is_some_and(|(_, rest)| rest == host)
        };
        if site == Some("cross-site") || origin.is_some_and(|o| !same(o)) {
            return AppError::Forbidden.into_response();
        }
    }
    if write {
        // Before and after: a read during the write must not cache the old row for long.
        // A member's own write only changes their own row, so only their sessions go (every
        // device, so revoking a session or changing a password still takes effect at once).
        // Staff, forum moderators (club owners among them), API keys and writers whose session
        // is not cached may change anyone's row, so the whole cache goes. Emptying it on every
        // write would leave it empty on a busy forum, and every request would load its session.
        let own = if req.headers().contains_key(axum::http::header::AUTHORIZATION) {
            None
        } else {
            crate::http::viewer::cookie(req.headers(), crate::http::viewer::SESSION_COOKIE)
                .and_then(|t| state.sessions.get(&bf_core::auth::token_digest(t)))
                .filter(|u| {
                    !u.is_admin
                        && !u.groups.contains(&bf_core::ids::GroupId::MODERATORS)
                        && !u.groups.contains(&bf_core::ids::GroupId::ADMINISTRATORS)
                        && u.moderated_nodes.is_empty()
                })
                .map(|u| u.id)
        };
        let forget = |state: &AppState| match own {
            Some(id) => forget_sessions_of(state, id.get()),
            None => state.sessions.invalidate_all(),
        };
        forget(&state);
        let res = next.run(req).await;
        forget(&state);
        return res;
    }
    next.run(req).await
}

/// Records server errors for the admin log. 503s from offline mode are expected.
pub async fn error_logger(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let path = bf_core::log::redact_path(req.uri().path()); // no query string: it can hold tokens
    let id = req.headers().get("x-request-id").and_then(|h| h.to_str().ok()).map(str::to_owned);
    let res = next.run(req).await;
    let status = res.status();
    if status.is_server_error()
        && status != StatusCode::SERVICE_UNAVAILABLE
        && let Some(pool) = state.pool.clone()
    {
        tokio::spawn(async move {
            if let Err(e) =
                bf_db::admin::maintenance::log_error(&pool, &method, &path, i32::from(status.as_u16()), id.as_deref())
                    .await
            {
                tracing::warn!(error = %e, "error log failed");
            }
        });
    }
    res
}

/// Offline mode: everyone but administrators gets a 503 page. Signing in, assets and
/// the admin API stay reachable so an admin can turn it off again.
/// Banned IPs may read but not write.
pub async fn ip_ban_gate(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Browser identifiers the administrator listed (scrapers, abusive tools) are refused outright.
    let listed = state.settings().str("blocked_user_agents").to_lowercase();
    if !listed.trim().is_empty()
        && req.uri().path() != "/robots.txt"
        && let Some(agent) = req.headers().get(axum::http::header::USER_AGENT).and_then(|h| h.to_str().ok())
    {
        let agent = agent.to_lowercase();
        if listed.split(['\n', ',']).map(str::trim).filter(|w| !w.is_empty()).any(|w| agent.contains(w)) {
            return AppError::Forbidden.into_response();
        }
    }
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let ip = req
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| crate::http::ratelimit::client_ip(c.0.ip(), req.headers()));
        if let (Some(ip), Some(pool)) = (ip, &state.pool)
            && bf_db::moderation::bans::ip_banned(pool, ip).await.unwrap_or(false)
        {
            return AppError::Forbidden.into_response();
        }
        // The operator's number comes from a trusted CDN header, e.g. BF_ASN_HEADER=CF-IPASN.
        if let (Ok(name), Some(pool)) = (std::env::var("BF_ASN_HEADER"), &state.pool) {
            let asn = req.headers().get(name.as_str()).and_then(|h| h.to_str().ok()).map(|a| a.trim().to_lowercase());
            if let Some(asn) = asn {
                let bare = asn.trim_start_matches("as").to_owned();
                if bf_db::moderation::bans::asn_banned(pool, &bare).await.unwrap_or(false)
                    || bf_db::moderation::bans::asn_banned(pool, &format!("as{bare}")).await.unwrap_or(false)
                {
                    return AppError::Forbidden.into_response();
                }
            }
        }
    }
    next.run(req).await
}

/// Every `/api/v1/admin/...` call needs an administrator (session, admin-scope key or the
/// bootstrap token) before any handler runs. Handlers check too, but only after their query and
/// body are decoded: without this, a stranger's malformed request got a 400 naming the fields,
/// mapping the admin API without any right to use it. Writes still need CSRF and sudo in the
/// handler (`admin(…, true)`).
pub async fn admin_api_gate(State(state): State<AppState>, v: Viewer, req: Request, next: Next) -> Response {
    if !req.uri().path().starts_with("/api/v1/admin") {
        return next.run(req).await;
    }
    match crate::admin::api::admin(&state, &v, req.headers(), false) {
        Ok(_) => next.run(req).await,
        // Asked to set up two-factor first: the panel shows that message, so it passes through.
        Err(AppError::Validation(m)) if v.user.as_ref().is_some_and(|u| u.is_admin) => {
            AppError::Validation(m).into_response()
        }
        Err(_) => AppError::Forbidden.into_response(),
    }
}

/// `login_required`: guests may only reach what it takes to get an account or sign in.
/// Every change made in a session an administrator opened as a member leaves a line in the audit
/// log under the administrator, next to whatever the action itself wrote as the member, so the
/// two can be matched by time and target. Reads are not logged.
pub async fn impersonation_trail(State(state): State<AppState>, v: Viewer, req: Request, next: Next) -> Response {
    let Some(admin) = v.impersonator() else { return next.run(req).await };
    let (method, path) = (req.method().clone(), req.uri().path().to_owned());
    let res = next.run(req).await;
    if !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS)
        && let (Some(pool), Some(user)) = (&state.pool, &v.user)
    {
        let details = serde_json::json!({
            "impersonated_by": admin,
            "method": method.as_str(),
            "path": path,
            "status": res.status().as_u16(),
        });
        let logged = bf_db::admin::audit::record(
            pool,
            bf_db::admin::audit::Entry::new("impersonated.request")
                .by(admin)
                .on(format!("user:{}", user.id))
                .details(details),
        )
        .await;
        if let Err(e) = logged {
            tracing::warn!(error = %e, "impersonation audit failed");
        }
    }
    res
}

pub async fn login_gate(State(state): State<AppState>, v: Viewer, req: Request, next: Next) -> Response {
    let read = req.method() == Method::GET || req.method() == Method::HEAD;
    if !read || v.user.is_some() || v.actor.is_admin || !state.settings().bool("login_required") {
        return next.run(req).await;
    }
    let path = req.uri().path();
    let open = [
        "/assets/",
        "/login",
        "/register",
        "/forgot",
        "/reset/",
        "/verify/",
        "/confirm-email/",
        "/auth/",
        "/legal/",
        "/unsubscribe/",
        "/language/",
        "/api/v1/passkeys",
        "/api/v1/admin",
        "/.well-known/",
    ]
    .iter()
    .any(|p| path.starts_with(p))
        || matches!(
            path,
            "/healthz" | "/livez" | "/readyz" | "/metrics" | "/robots.txt" | "/manifest.webmanifest" | "/sw.js"
        );
    if open {
        return next.run(req).await;
    }
    if path.starts_with("/api/") {
        return AppError::Forbidden.into_response();
    }
    axum::response::Redirect::to("/login").into_response()
}

/// Paths that answer machines, not browsers: their errors stay problem+json.
fn machine_path(path: &str) -> bool {
    ["/api/", "/ap/", "/.well-known/", "/nodeinfo/", "/oembed", "/newsletter/", "/uploads/", "/assets/"]
        .iter()
        .any(|p| path.starts_with(p))
        || matches!(path, "/healthz" | "/livez" | "/readyz" | "/metrics")
}

/// Errors as pages: handlers return `AppError`, which is problem+json for the API and
/// the islands' `fetch` calls. A browser navigating to a page (it asks for `text/html`) gets
/// the site's error page instead, with the same status and message. A guest refused a page is
/// sent to sign in, and a member who must confirm their password is sent to do that.
pub async fn html_errors(State(state): State<AppState>, v: Viewer, req: Request, next: Next) -> Response {
    use axum::http::header;
    let wants_html =
        req.headers().get(header::ACCEPT).and_then(|a| a.to_str().ok()).is_some_and(|a| a.contains("text/html"));
    let path = req.uri().path().to_owned();
    let path_q = req.uri().path_and_query().map_or("/", |p| p.as_str()).to_owned();
    // The viewer's language for everything this request builds (`http::i18n::tr`).
    let res = crate::http::i18n::with_locale(v.t(), next.run(req)).await;
    let problem = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|c| c.to_str().ok())
        .is_some_and(|c| c.starts_with("application/problem+json"));
    if !problem {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let body = axum::body::to_bytes(body, 64 * 1024).await.unwrap_or_default();
    let mut p: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    // Errors are written in English in the code (`AppError`); each viewer reads them in their own
    // language, in a page or in the JSON an island or the admin panel shows.
    let t = v.t();
    for field in ["detail", "title"] {
        if let Some(text) = p[field].as_str() {
            let translated = t.msg(text);
            p[field] = translated.into();
        }
    }
    // An error with values in its text: the template is translated, then filled in.
    if let Some(i18n) = p.get("i18n").cloned() {
        if let (Some(template), Some(vars)) = (i18n["template"].as_str(), i18n["vars"].as_object()) {
            let pairs: Vec<(&str, &str)> =
                vars.iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap_or_default())).collect();
            p["detail"] = t.msgf(template, &pairs).into();
        }
        if let Some(o) = p.as_object_mut() {
            o.remove("i18n");
        }
    }
    if !wants_html || machine_path(&path) {
        parts.headers.remove(header::CONTENT_LENGTH);
        return Response::from_parts(parts, axum::body::Body::from(serde_json::to_vec(&p).unwrap_or_default()));
    }
    let status = parts.status;
    let retry = parts.headers.get(header::RETRY_AFTER).cloned();
    let kind = p["type"].as_str().unwrap_or_default();
    if kind == "sudo-required" {
        let next = bf_core::html::url_encode_keeping(&path_q, b"/");
        return axum::response::Redirect::to(&format!("/sudo?next={next}")).into_response();
    }
    if status == StatusCode::FORBIDDEN && v.user.is_none() {
        return axum::response::Redirect::to("/login").into_response();
    }
    let message = match (status.as_u16(), p["detail"].as_str()) {
        (_, Some(d)) if !d.trim().is_empty() => d.to_owned(),
        (404, _) => t.msg("That page does not exist, or you cannot see it."),
        (403, _) => t.msg("You do not have permission to do that."),
        (429, _) => t.msg("Too many requests. Wait a moment and try again."),
        (400 | 422, _) => t.msg("That did not work. Go back, check what you entered and try again."),
        (s, _) if s >= 500 => t.msg("Something went wrong on our side. Please try again in a moment."),
        _ => p["title"].as_str().map_or_else(|| t.msg("Something went wrong"), str::to_owned),
    };
    let title = match status.as_u16() {
        404 => t.msg("Not found"),
        403 => t.msg("Not allowed"),
        422 | 400 => t.msg("Please check that"),
        429 => t.msg("Slow down"),
        _ => t.msg("Error"),
    };
    let (layout, set) = crate::http::render::layout(&state, &v, Some(&title));
    let page = bf_web::pages::ErrorPage { layout, status: status.as_u16(), message };
    let mut out = (status, bf_web::pages::render(&page)).into_response();
    if let Some(c) = set {
        out.headers_mut().append(header::SET_COOKIE, c);
    }
    if let Some(r) = retry {
        out.headers_mut().insert(header::RETRY_AFTER, r);
    }
    out.headers_mut().insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("private, no-store"));
    out
}

pub async fn offline_gate(State(state): State<AppState>, v: Viewer, req: Request, next: Next) -> Response {
    if !state.settings().bool("offline") || v.actor.is_admin {
        return next.run(req).await;
    }
    let path = req.uri().path();
    let open = path.starts_with("/assets/")
        || path.starts_with("/login")
        || path.starts_with("/auth/")
        || path.starts_with("/forgot")
        || path.starts_with("/reset/")
        || path.starts_with("/api/v1/passkeys")
        || path.starts_with("/api/v1/admin")
        || matches!(path, "/healthz" | "/livez" | "/readyz" | "/metrics")
        || (req.method() == Method::POST && path == "/logout");
    if open {
        return next.run(req).await;
    }
    let s = state.settings();
    let mut layout = bf_web::pages::Layout::new(&s, Some(&v.t().msg("Offline")));
    layout.t = v.t();
    layout.set_nav(&s, &bf_core::ids::raw(&v.actor.groups), v.actor.is_admin);
    let page = bf_web::pages::ErrorPage { layout, status: 503, message: s.str("offline_message").to_owned() };
    let mut res = (StatusCode::SERVICE_UNAVAILABLE, bf_web::pages::render(&page)).into_response();
    res.headers_mut().insert(axum::http::header::RETRY_AFTER, axum::http::HeaderValue::from_static("300"));
    res
}

/// Browsers report CSP violations here: `application/csp-report` from `report-uri`, and
/// `application/reports+json` batches from the Reporting API. Logged in short form and
/// counted by directive, never stored.
async fn csp_report(body: axum::body::Bytes) -> StatusCode {
    if body.len() > 64 * 1024 {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    // A Reporting API batch is an array of {type, url, body}; the old format is {"csp-report": {...}}.
    let reports: Vec<&serde_json::Value> = match &parsed {
        serde_json::Value::Array(items) => {
            items.iter().filter(|r| r["type"] == "csp-violation").map(|r| &r["body"]).take(20).collect()
        }
        v => v.get("csp-report").into_iter().collect(),
    };
    for r in &reports {
        let directive = r
            .get("effectiveDirective")
            .or_else(|| r.get("effective-directive"))
            .or_else(|| r.get("violated-directive"))
            .and_then(|d| d.as_str())
            .and_then(|d| d.split_whitespace().next())
            .filter(|d| d.len() <= 40 && d.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
            .unwrap_or("unknown")
            .to_owned();
        metrics::counter!("bf_csp_violations_total", "directive" => directive).increment(1);
    }
    if reports.is_empty() {
        metrics::counter!("bf_csp_violations_total", "directive" => "unparsed").increment(1);
    }
    let text: String = String::from_utf8_lossy(&body).chars().take(500).collect();
    tracing::warn!(report = %text, "csp violation");
    StatusCode::NO_CONTENT
}

/// Drops every cached session of one member, so the next request loads their row again.
pub(crate) fn forget_sessions_of(state: &AppState, user: i64) {
    if state.sessions.invalidate_entries_if(move |_, u| u.id == user).is_err() {
        state.sessions.invalidate_all();
    }
}

#[cfg(test)]
mod tests {
    use super::request_is_secure;
    use bf_core::config::HttpsRedirect::{Always, Proxied};

    #[test]
    fn direct_peer_cannot_claim_https() {
        assert!(!request_is_secure(Always, false, false, Some("https")));
    }

    #[test]
    fn trusted_proxy_header_is_honoured() {
        assert!(request_is_secure(Always, false, true, Some("https")));
        assert!(request_is_secure(Always, false, true, Some("HTTPS, http")));
        assert!(!request_is_secure(Always, false, true, Some("http")));
        assert!(!request_is_secure(Proxied, false, true, Some("http")));
    }
}
