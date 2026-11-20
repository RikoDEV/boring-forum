//! Router tests that need no database: pages, assets, error model, admin gating.

#![expect(clippy::expect_used, reason = "test helpers: a failed setup is the failure report")]

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use bf_core::{config::Config, settings::Settings};
use bf_server::{app, state::AppState};
use tower::ServiceExt;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn state(admin_token: Option<&str>, rate_limit_per_min: u32) -> AppState {
    let config = Config {
        bind: ([127, 0, 0, 1], 0).into(),
        database_url: String::new(),
        db_pool_size: 1,
        microcache_ttl_ms: 1000,
        rate_limit_per_min,
        log_json: false,
        admin_token: admin_token.map(str::to_owned),
        admin_dir: "../../web/admin/build".into(),
        base_url: "http://forum.test".into(),
        tls_cert: None,
        tls_key: None,
        https_redirect: bf_core::config::HttpsRedirect::Off,
        hsts_preload: false,
    };
    AppState::build(config, None, Settings::default())
}

async fn body(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.expect("the response body is in memory");
    String::from_utf8(bytes.to_vec()).expect("the response body is UTF-8")
}

#[tokio::test]
async fn home_renders_with_request_id_and_security_headers() {
    let res = app(state(None, 100)).oneshot(Request::get("/").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().contains_key("x-request-id"));
    assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert!(body(res).await.contains("<title>Boring Forum</title>"));
}

#[tokio::test]
async fn hashed_assets_are_immutable() {
    let url = bf_web::assets::CSS.url();
    let res = app(state(None, 100)).oneshot(Request::get(&url).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers()[header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));
}

#[tokio::test]
async fn unknown_api_route_is_problem_json() {
    let res = app(state(None, 100)).oneshot(Request::get("/api/v1/nope").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "application/problem+json");
}

#[tokio::test]
async fn unknown_page_is_html_404() {
    let res = app(state(None, 100)).oneshot(Request::get("/missing").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert!(body(res).await.contains("That page does not exist."));
}

#[tokio::test]
async fn admin_requires_token() {
    let get = |auth: Option<&str>| {
        let mut req = Request::get("/api/v1/admin/settings");
        if let Some(a) = auth {
            req = req.header(header::AUTHORIZATION, a);
        }
        req.body(Body::empty()).unwrap()
    };
    // Disabled entirely without a configured token.
    let res = app(state(None, 100)).oneshot(get(Some(&format!("Bearer {TOKEN}")))).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    let app = app(state(Some(TOKEN), 100));
    assert_eq!(app.clone().oneshot(get(None)).await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(app.clone().oneshot(get(Some("Bearer wrong"))).await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(app.oneshot(get(Some(&format!("Bearer {TOKEN}")))).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_setting_update_validates_and_applies() {
    let st = state(Some(TOKEN), 100);
    let put = |value: &str| {
        Request::put("/api/v1/admin/settings/site_name")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(r#"{{"value":{value}}}"#)))
            .unwrap()
    };
    let res = app(st.clone()).oneshot(put("42")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let res = app(st.clone()).oneshot(put(r#""Quiet Place""#)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(st.settings().str("site_name"), "Quiet Place");

    let res = app(st).oneshot(Request::get("/").body(Body::empty()).unwrap()).await.unwrap();
    assert!(body(res).await.contains("Quiet Place"));
}

#[tokio::test]
async fn rate_limit_returns_429() {
    let app = app(state(None, 2));
    for _ in 0..2 {
        let res = app.clone().oneshot(Request::get("/missing").body(Body::empty()).unwrap()).await.unwrap();
        assert_ne!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    }
    let res = app.clone().oneshot(Request::get("/missing").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().contains_key(header::RETRY_AFTER));
    // Probes are never limited: an orchestrator checks from one address, often.
    for path in ["/healthz", "/livez", "/readyz"] {
        let res = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        assert_ne!(res.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
    }
}

#[tokio::test]
async fn healthz_needs_no_database_and_readyz_does() {
    // No pool: the process is alive, but it is not ready to serve.
    let res = app(state(None, 100)).oneshot(Request::get("/healthz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(body(res).await.contains("\"ok\""));

    let res = app(state(None, 100)).oneshot(Request::get("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value = serde_json::from_str(&body(res).await).unwrap();
    assert_eq!(json["status"], "unavailable");
    assert_eq!(json["db"], false);
}

#[tokio::test]
async fn metrics_is_not_public() {
    // No token configured: only a loopback peer may scrape, and a test request has no peer.
    let res = app(state(None, 100)).oneshot(Request::get("/metrics").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_spa_is_not_served_to_guests() {
    // `admin_gate` answers anyone but an administrator with the site's 404 page: no redirect to
    // /login and nothing that confirms the panel exists.
    for path in ["/admin/users", "/admin/"] {
        let res = app(state(None, 100)).oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!res.headers().contains_key(header::LOCATION), "{path} must not redirect to a login");
    }
}

#[tokio::test]
async fn login_page_sets_csrf_cookie_and_is_private() {
    let res = app(state(None, 100)).oneshot(Request::get("/login").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res.headers()[header::SET_COOKIE].to_str().unwrap().to_owned();
    assert!(cookie.starts_with("__Host-bf_csrf=") && cookie.contains("Secure"));
    assert!(res.headers()[header::CACHE_CONTROL].to_str().unwrap().contains("no-store"));
    let token = &cookie["__Host-bf_csrf=".len()..cookie.find(';').unwrap()];
    assert!(body(res).await.contains(token));
}

#[tokio::test]
async fn post_without_matching_csrf_is_rejected() {
    let form = |csrf: &str, cookie: &str| {
        Request::post("/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::COOKIE, cookie)
            .body(Body::from(format!("csrf={csrf}&login=a&password=b")))
            .unwrap()
    };
    let t = "a".repeat(64);
    let res = app(state(None, 100)).oneshot(form("", &format!("__Host-bf_csrf={t}"))).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = app(state(None, 100)).oneshot(form(&t, "")).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn member_only_pages_reject_guests() {
    for path in ["/notifications", "/c/1/new"] {
        let res = app(state(None, 100)).oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn preview_requires_a_member() {
    let req = Request::post("/api/v1/preview")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"raw":"**hi**"}"#))
        .unwrap();
    assert_eq!(app(state(None, 100)).oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn editor_bundle_is_served_immutable() {
    let res = app(state(None, 100))
        .oneshot(Request::get(bf_web::assets::EDITOR.url()).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers()[header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));
}

#[tokio::test]
async fn robots_txt_points_at_sitemap() {
    let res = app(state(None, 100)).oneshot(Request::get("/robots.txt").body(Body::empty()).unwrap()).await.unwrap();
    assert!(body(res).await.contains("Sitemap: http://forum.test/sitemap.xml"));
}

#[test]
fn openapi_lists_endpoints() {
    let spec = bf_server::platform::openapi::spec_json();
    assert!(spec.contains("/api/v1/settings"));
    assert!(spec.contains("/api/v1/admin/settings/{key}"));
}
