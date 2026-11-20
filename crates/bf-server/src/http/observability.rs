//! Observability: Prometheus metrics, liveness and readiness probes, W3C trace context
//! and optional OTLP trace export.
//!
//! `/metrics` is for the operator's scraper, never for the public: it answers a bearer token
//! from `BF_METRICS_TOKEN`, or, with no token configured, only a loopback peer whose request was
//! not forwarded by a proxy (no `X-Forwarded-For`, `Forwarded` or `X-Real-IP`).

use crate::state::AppState;
use axum::Router;
use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/metrics", get(scrape))
        .route("/healthz", get(livez))
        .route("/livez", get(livez))
        .route("/readyz", get(readyz))
}

/// Set when a stop signal arrives: `/readyz` then answers 503 so the load balancer stops sending
/// traffic while requests already in flight finish.
static DRAINING: AtomicBool = AtomicBool::new(false);

/// How long readiness reports "draining" before the listener stops accepting, in seconds
/// (`BF_DRAIN_SECS`, default 5), so a load balancer has time to notice.
fn drain_delay() -> Duration {
    Duration::from_secs(std::env::var("BF_DRAIN_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(5).min(120))
}

/// Resolves on SIGTERM or Ctrl-C, after the drain delay. Pass it to `with_graceful_shutdown`.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    #[expect(clippy::expect_used, reason = "startup: a process that cannot install signal handlers must not start")]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("a SIGTERM handler can be installed")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = term => {} }
    DRAINING.store(true, Ordering::SeqCst);
    tracing::info!("stop signal received; draining");
    tokio::time::sleep(drain_delay()).await;
}

/// `boring-forum healthcheck`: asks this instance's own `/readyz` over loopback, for the image's
/// HEALTHCHECK (the image has no curl). True on a 200.
pub async fn probe_self() -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let bind = std::env::var("BF_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let port = bind.rsplit(':').next().unwrap_or("8080").to_owned();
    let probe = async {
        let mut conn = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await.ok()?;
        conn.write_all(b"GET /readyz HTTP/1.0\r\nHost: localhost\r\n\r\n").await.ok()?;
        let mut head = [0u8; 16];
        conn.read_exact(&mut head).await.ok()?;
        Some(head.starts_with(b"HTTP/1.1 200") || head.starts_with(b"HTTP/1.0 200"))
    };
    tokio::time::timeout(Duration::from_secs(3), probe).await.ok().flatten().unwrap_or(false)
}

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Latency buckets in seconds, around the budgets (p95 of 50 ms for pages).
const BUCKETS: &[f64] = &[0.002, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 10.0];

/// Installs the global recorder once. Later calls (tests building several apps) are no-ops.
#[expect(clippy::expect_used, reason = "startup: the bucket list is a non-empty constant")]
pub fn install() {
    HANDLE.get_or_init(|| {
        let recorder =
            PrometheusBuilder::new().set_buckets(BUCKETS).expect("static buckets are valid").build_recorder();
        let handle = recorder.handle();
        // Another recorder may already be set by a test harness; metrics are then not recorded.
        let _ = metrics::set_global_recorder(recorder);
        let upkeep = handle.clone();
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
                loop {
                    tick.tick().await;
                    upkeep.run_upkeep();
                }
            });
        }
        handle
    });
}

/// Request count and latency by route template, never by raw path: ids in paths would give
/// every topic its own series.
pub async fn track(req: Request, next: Next) -> Response {
    let route = req.extensions().get::<MatchedPath>().map_or("unmatched", |p| p.as_str()).to_owned();
    let method = req.method().as_str().to_owned();
    let started = Instant::now();
    let res = next.run(req).await;
    let status = res.status().as_u16().to_string();
    let secs = started.elapsed().as_secs_f64();
    metrics::counter!("bf_http_requests_total", "method" => method.clone(), "route" => route.clone(), "status" => status)
        .increment(1);
    metrics::histogram!("bf_http_request_duration_seconds", "method" => method, "route" => route).record(secs);
    res
}

/// Whether the request names this machine (`localhost`, `127.0.0.1`, `::1`, with or without a
/// port). A proxy that passes the visitor's `Host` on to us carries the public name instead.
fn loopback_host(headers: &HeaderMap, uri: &axum::http::Uri) -> bool {
    let local = |h: &str| {
        let h = h.trim();
        let name = match h.strip_prefix('[') {
            Some(rest) => rest.split(']').next().unwrap_or(""),
            None if h.matches(':').count() > 1 => h,
            None => h.split(':').next().unwrap_or(""),
        };
        name.eq_ignore_ascii_case("localhost") || name.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    };
    headers.get_all(header::HOST).iter().all(|h| h.to_str().is_ok_and(local))
        && uri.authority().is_none_or(|a| local(a.as_str()))
}

fn allowed(headers: &HeaderMap, uri: &axum::http::Uri, peer: Option<SocketAddr>) -> bool {
    match std::env::var("BF_METRICS_TOKEN").ok().filter(|t| t.len() >= 16) {
        Some(token) => headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .is_some_and(|given| bf_core::auth::constant_eq(given.as_bytes(), token.as_bytes())),
        // A reverse proxy on this host is a loopback peer too, for every visitor it forwards. A
        // request that says it was forwarded is one of those, not the operator's own scraper.
        None => {
            peer.is_some_and(|p| p.ip().is_loopback())
                && !["x-forwarded-for", "forwarded", "x-real-ip"].iter().any(|h| headers.contains_key(*h))
                // A same-host proxy that adds none of those headers still passes the public `Host`.
                && loopback_host(headers, uri)
        }
    }
}

async fn scrape(State(state): State<AppState>, headers: HeaderMap, req: Request) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    if !allowed(&headers, req.uri(), peer) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(handle) = HANDLE.get() else { return StatusCode::SERVICE_UNAVAILABLE.into_response() };
    if let Some(pool) = &state.pool {
        metrics::gauge!("bf_db_pool_connections").set(f64::from(pool.size()));
        metrics::gauge!("bf_db_pool_idle").set(pool.num_idle() as f64);
        match bf_db::admin::maintenance::queue_depths(pool).await {
            Ok(depths) => {
                for (queue, n) in depths {
                    metrics::gauge!("bf_queue_depth", "queue" => queue).set(n as f64);
                }
            }
            Err(e) => tracing::warn!(error = %e, "queue depth read failed"),
        }
    }
    metrics::gauge!("bf_realtime_subscribers").set(state.hub.subscriber_count() as f64);
    let body = handle.render();
    ([(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"))], body)
        .into_response()
}

/// Liveness: the process answers. No I/O at all, so a database outage does not get every
/// instance restarted at once.
async fn livez() -> Response {
    (StatusCode::OK, axum::Json(serde_json::json!({ "status": "ok" }))).into_response()
}

/// Readiness: the database answers within two seconds, the schema is at least what this binary
/// expects, the required configuration is present, and the process is not draining.
async fn readyz(State(state): State<AppState>) -> Response {
    let draining = DRAINING.load(Ordering::SeqCst);
    let config = !state.config.base_url.is_empty();
    let (db, migrations) = match &state.pool {
        Some(pool) => {
            let check = async {
                if bf_db::ping(pool).await.is_err() {
                    return (false, false);
                }
                (true, bf_db::migrations_current(pool).await.unwrap_or(false))
            };
            tokio::time::timeout(Duration::from_secs(2), check).await.unwrap_or((false, false))
        }
        None => (false, false),
    };
    let ready = db && migrations && config && !draining;
    let status = if ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    let body = serde_json::json!({
        "status": if ready { "ok" } else { "unavailable" },
        "db": db,
        "migrations": migrations,
        "config": config,
        "draining": draining,
    });
    let mut res = (status, axum::Json(body)).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// A W3C `traceparent` (version 00) as (trace id, parent span id), when it is well formed.
pub fn parse_traceparent(value: &str) -> Option<(&str, &str)> {
    let mut parts = value.trim().split('-');
    let (version, trace, span, flags) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let ok = version == "00"
        && parts.next().is_none()
        && hex(trace, 32)
        && hex(span, 16)
        && hex(flags, 2)
        && trace.bytes().any(|b| b != b'0')
        && span.bytes().any(|b| b != b'0');
    ok.then_some((trace, span))
}

/// Headers that carry the current trace to an outbound request. Empty without OTLP export,
/// where there is no span context to pass on.
pub fn trace_headers() -> Vec<(String, String)> {
    use opentelemetry::propagation::TextMapPropagator;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    if !OTEL_ON.get().copied().unwrap_or(false) {
        return Vec::new();
    }
    let cx = tracing::Span::current().context();
    let mut out = std::collections::HashMap::new();
    opentelemetry_sdk::propagation::TraceContextPropagator::new().inject_context(&cx, &mut out);
    out.into_iter().collect()
}

/// Makes the request span a child of the caller's trace, when OTLP export is on.
pub fn adopt_parent(span: &tracing::Span, headers: &HeaderMap) {
    use opentelemetry::propagation::TextMapPropagator;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    if !OTEL_ON.get().copied().unwrap_or(false) {
        return;
    }
    let Some(tp) = headers.get("traceparent").and_then(|v| v.to_str().ok()).filter(|v| parse_traceparent(v).is_some())
    else {
        return;
    };
    let mut carrier = std::collections::HashMap::new();
    carrier.insert("traceparent".to_owned(), tp.to_owned());
    if let Some(ts) = headers.get("tracestate").and_then(|v| v.to_str().ok()) {
        carrier.insert("tracestate".to_owned(), ts.chars().take(512).collect());
    }
    let cx = opentelemetry_sdk::propagation::TraceContextPropagator::new().extract(&carrier);
    let _ = span.set_parent(cx);
}

static OTEL_ON: OnceLock<bool> = OnceLock::new();

/// An OTLP/HTTP trace layer when `OTEL_EXPORTER_OTLP_ENDPOINT` is set (the standard variable;
/// `OTEL_SERVICE_NAME` and the other `OTEL_*` variables are read by the SDK).
// The tracing subscriber this layer belongs to is not installed yet, so stderr is the only channel.
#[allow(clippy::print_stderr)]
pub fn otel_layer<S>() -> Option<impl tracing_subscriber::Layer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use opentelemetry::trace::TracerProvider as _;
    std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok().filter(|v| !v.is_empty())?;
    let exporter = match opentelemetry_otlp::SpanExporter::builder().with_http().build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("OTLP exporter could not start: {e}");
            return None;
        }
    };
    let name = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "boring-forum".into());
    let resource = opentelemetry_sdk::Resource::builder().with_service_name(name).build();
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();
    let tracer = provider.tracer("boring-forum");
    opentelemetry::global::set_tracer_provider(provider);
    let _ = OTEL_ON.set(true);
    Some(tracing_opentelemetry::layer().with_tracer(tracer))
}

#[cfg(test)]
mod tests {
    use super::parse_traceparent;

    #[test]
    fn traceparent_is_checked() {
        let ok = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        assert_eq!(parse_traceparent(ok), Some(("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7")));
        assert!(parse_traceparent("00-00000000000000000000000000000000-00f067aa0ba902b7-01").is_none());
        assert!(parse_traceparent("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").is_none());
        assert!(parse_traceparent("00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01").is_none());
        assert!(parse_traceparent("garbage").is_none());
    }
}
