//! Benchmark harness for the server budgets. Drives the real router in-process
//! (no network, no database) and exits non-zero when a budget is missed.
//!
//! Run in release mode: `cargo run --release -p bf-bench`.
//! Page budgets that need content (topic pages) can be added here.

// This binary exists to print its report; there is no logger to route it through.
#![allow(clippy::print_stdout, clippy::print_stderr)]
#![expect(clippy::expect_used, reason = "a benchmark harness: a failed request or task aborts the run")]

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use bf_core::{config::Config, settings::Settings};
use bf_server::{app, state::AppState};
use std::time::{Duration, Instant};
use tower::ServiceExt;

struct Case {
    name: &'static str,
    path: &'static str,
    /// Members bypass the microcache, so this measures a real render.
    member: bool,
    p95_budget: Duration,
}

const CASES: &[Case] = &[
    // A hit skips the render but not the outer layers (compression, link signing, traffic): measured
    // p95 3.6 ms at 64 concurrent requests on a workstation; 8 ms leaves room for a shared CI runner.
    Case { name: "guest home (microcache)", path: "/", member: false, p95_budget: Duration::from_millis(8) },
    Case { name: "member home (render)", path: "/", member: true, p95_budget: Duration::from_millis(15) },
];

const REQUESTS: usize = 20_000;
const CONCURRENCY: usize = 64;

#[tokio::main]
async fn main() {
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; numbers are not meaningful (use --release)");
    }
    let config = Config {
        bind: ([127, 0, 0, 1], 0).into(),
        database_url: String::new(),
        db_pool_size: 1,
        microcache_ttl_ms: 2000,
        rate_limit_per_min: u32::MAX,
        log_json: false,
        admin_token: None,
        admin_dir: "web/admin/build".into(),
        base_url: "http://forum.test".into(),
        tls_cert: None,
        tls_key: None,
        https_redirect: bf_core::config::HttpsRedirect::Off,
        hsts_preload: false,
    };
    let router = app(AppState::build(config, None, Settings::default()));

    let mut failed = false;
    for case in CASES {
        let (latencies, elapsed) = run(&router, case).await;
        let p = |q: f64| latencies[((latencies.len() as f64 - 1.0) * q) as usize];
        let (p50, p95, p99) = (p(0.50), p(0.95), p(0.99));
        let ok = p95 <= case.p95_budget;
        failed |= !ok;
        println!(
            "{} {:<26} {:>9.0} req/s  p50 {:>8.1?}  p95 {:>8.1?}  p99 {:>8.1?}  (p95 budget {:?})",
            if ok { "ok  " } else { "FAIL" },
            case.name,
            REQUESTS as f64 / elapsed.as_secs_f64(),
            p50,
            p95,
            p99,
            case.p95_budget,
        );
    }
    std::process::exit(i32::from(failed));
}

async fn run(router: &axum::Router, case: &Case) -> (Vec<Duration>, Duration) {
    let request = || {
        let mut req = Request::get(case.path);
        if case.member {
            req = req.header(header::COOKIE, "__Host-bf_session=bench");
        }
        req.body(Body::empty()).expect("the benchmark request is well formed")
    };
    // Warm up caches and lazy statics.
    for _ in 0..100 {
        router.clone().oneshot(request()).await.expect("the router is infallible");
    }
    let start = Instant::now();
    let per_task = REQUESTS / CONCURRENCY;
    let tasks: Vec<_> = (0..CONCURRENCY)
        .map(|_| {
            let router = router.clone();
            let reqs: Vec<_> = (0..per_task).map(|_| request()).collect();
            tokio::spawn(async move {
                let mut out = Vec::with_capacity(per_task);
                for req in reqs {
                    let t = Instant::now();
                    let res = router.clone().oneshot(req).await.expect("the router is infallible");
                    assert_eq!(res.status(), StatusCode::OK);
                    axum::body::to_bytes(res.into_body(), usize::MAX).await.expect("the response body can be read");
                    out.push(t.elapsed());
                }
                out
            })
        })
        .collect();
    let mut all = Vec::with_capacity(REQUESTS);
    for t in tasks {
        all.extend(t.await.expect("a benchmark task does not panic"));
    }
    let elapsed = start.elapsed();
    all.sort_unstable();
    (all, elapsed)
}
