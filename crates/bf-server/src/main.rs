//! The `boring-forum` binary: the server and its command-line subcommands.

// The subcommands (`openapi`, `vapid`, `secret-key`, `backup`, ...) answer on stdout and stderr.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use bf_core::config::Config;
use bf_server::platform::openapi;
use bf_server::{app, state::AppState};
use std::net::SocketAddr;
use tracing_subscriber::EnvFilter;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() {
    // `boring-forum openapi` prints the spec for the TypeScript client generator.
    if std::env::args().nth(1).as_deref() == Some("openapi") {
        println!("{}", openapi::spec_json());
        return;
    }
    // `boring-forum healthcheck` is the image's HEALTHCHECK: exit 0 when /readyz answers 200.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(i32::from(!bf_server::http::observability::probe_self().await));
    }
    // `boring-forum --check-config` (or `check-config`) loads the environment as the server would
    // and says what is wrong or doubtful. Exit 1 on an error; warnings alone exit 0.
    if matches!(std::env::args().nth(1).as_deref(), Some("--check-config" | "check-config")) {
        match Config::from_env().and_then(|c| bf_core::secretbox::Keyring::from_env().map(|ring| (c, ring))) {
            Ok((c, ring)) => {
                if let Some(e) = c.secret_key_error(ring.is_some()) {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
                if ring.is_none() {
                    println!("warning: BF_SECRET_KEY is not set: TOTP and webhook secrets are stored in plain text");
                }
                for w in c.warnings() {
                    println!("warning: {w}");
                }
                println!("configuration loads");
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    // `boring-forum plugin new <id>` scaffolds a plugin.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [a, b, id] = args.as_slice()
        && a == "plugin"
        && b == "new"
    {
        match bf_server::platform::plugins::scaffold(id) {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    // `boring-forum plugin pack <dir> [out.zip]`, `plugin sign <zip> <seed-hex>` and
    // `plugin watch <dir> <site-url> <admin-key>`: build, sign and live-reinstall a plugin.
    if let [a, b, rest @ ..] = args.as_slice()
        && a == "plugin"
        && matches!(b.as_str(), "pack" | "sign" | "watch")
    {
        let done: Result<(), String> = match (b.as_str(), rest) {
            ("pack", [dir, out @ ..]) => bf_server::platform::plugins::pack(dir).and_then(|bytes| {
                let path = out.first().cloned().unwrap_or_else(|| format!("{}/dist/plugin.zip", dir.trim_end_matches('/')));
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
                println!("Wrote {path}");
                Ok(())
            }),
            ("sign", [zip, seed]) => std::fs::read(zip).map_err(|e| e.to_string()).and_then(|bytes| {
                std::fs::write(zip, bf_server::platform::plugins::sign(&bytes, seed)?).map_err(|e| e.to_string())?;
                println!("Signed {zip}");
                Ok(())
            }),
            ("watch", [dir, url, key]) => bf_server::platform::plugins::watch(dir, url, key).await,
            _ => Err("usage: plugin pack <dir> [out.zip] | plugin sign <zip> <seed-hex> | plugin watch <dir> <site-url> <admin-key>".into()),
        };
        if let Err(e) = done {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("vapid") {
        print!("{}", bf_server::platform::push::generate_keys());
        return;
    }
    // `boring-forum secret-key` prints a key for BF_SECRET_KEY (encryption at rest).
    if std::env::args().nth(1).as_deref() == Some("secret-key") {
        print!("{}", bf_core::secretbox::generate_key());
        return;
    }
    // `boring-forum rotate-secrets` re-encrypts the sensitive columns with the current key.
    if args.first().map(String::as_str) == Some("rotate-secrets") {
        if let Err(e) = rotate_secrets().await {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    // `boring-forum backup [dir]` writes a backup archive; `boring-forum restore <file> [--yes]`
    // replaces the database and uploads with one. Stop the server before a restore.
    if matches!(args.first().map(String::as_str), Some("backup" | "restore")) {
        if let Err(e) = backup_command(&args).await {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    // `boring-forum import <file.json>` loads a forum exported to the neutral format.
    if let [a, file] = args.as_slice()
        && a == "import"
    {
        match bf_server::platform::importer::run(file).await {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    // `boring-forum theme watch <dir> <site-url> <admin-key>` pushes a theme on every save.
    if let [a, b, dir, url, key] = args.as_slice()
        && a == "theme"
        && b == "watch"
    {
        if let Err(e) = bf_server::admin::themes::watch(dir, url, key).await {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = run().await {
        eprintln!("fatal: {e}");
        std::process::exit(1);
    }
}

/// Connects with the configured URL and re-seals every stale secret.
async fn backup_command(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use bf_server::platform::backup;
    let config = Config::from_env()?;
    let pool = bf_db::connect(&config.database_url, 2).await?;
    if args[0] == "backup" {
        let dir = args.get(1).map_or_else(backup::dir, std::path::PathBuf::from);
        let made = backup::create(&pool, &dir, Some(&backup::uploads_dir()), "command line").await?;
        println!("{}  ({} bytes)", dir.join(&made.name).display(), made.size);
        return Ok(());
    }
    let file = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .ok_or("usage: boring-forum restore <backup.tar.gz> [--yes]")?;
    if !args.iter().any(|a| a == "--yes") {
        eprintln!("This replaces every table in the database and adds the backup's uploads.");
        eprintln!("Stop the server first. Type restore to continue:");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim() != "restore" {
            return Err("cancelled".into());
        }
    }
    // The role that owns the tables, when one is configured.
    let ddl = bf_db::ddl_pool(&pool).await?;
    let manifest = backup::restore(&ddl, std::path::Path::new(file), &backup::uploads_dir()).await?;
    println!("restored the backup of {} ({} tables)", manifest.created_at, manifest.tables.len());
    Ok(())
}

async fn rotate_secrets() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::new("info")).init();
    let config = Config::from_env()?;
    let pool = bf_db::connect(&config.database_url, 2).await?;
    println!("{}", bf_db::platform::secrets::rotate(&pool).await?);
    Ok(())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));
    {
        use tracing_subscriber::prelude::*;
        let fmt = if config.log_json {
            tracing_subscriber::fmt::layer().json().boxed()
        } else {
            tracing_subscriber::fmt::layer().boxed()
        };
        // Spans also go to an OTLP collector when OTEL_EXPORTER_OTLP_ENDPOINT is set.
        tracing_subscriber::registry().with(filter).with(fmt).with(bf_server::http::observability::otel_layer()).init();
    }

    // Check the encryption keyring now rather than when the first secret is read, so a
    // bad key is a startup error like any other configuration mistake, and a missing one warns
    // where an operator will see it.
    let ring = bf_core::secretbox::Keyring::from_env()?;
    if let Some(e) = config.secret_key_error(ring.is_some()) {
        return Err(e.into());
    }
    let _ = bf_core::secretbox::keyring();
    for w in config.warnings() {
        tracing::warn!("{w}");
    }

    // Migrations run as a role that may change the schema, the server as one that may
    // not. With BF_MIGRATE_DATABASE_URL set, the DDL happens on a short-lived pool of its own
    // and is closed before the application pool is used.
    if let Some(url) = std::env::var("BF_MIGRATE_DATABASE_URL").ok().filter(|u| !u.is_empty()) {
        let migrator = bf_db::connect(&url, 1).await?;
        bf_db::migrate(&migrator).await?;
        migrator.close().await;
        tracing::info!("migrations applied with the migration role");
    } else {
        let pool = bf_db::connect(&config.database_url, 1).await?;
        bf_db::migrate(&pool).await?;
        pool.close().await;
    }
    let pool = bf_db::connect(&config.database_url, config.db_pool_size).await?;
    // The application must run as a least-privilege role, never a superuser. A
    // superuser connection turns any SQL-injection or Data Explorer escape into code execution on
    // the database host. Deploy with deploy/postgres/roles.sql (BF_DATABASE_URL -> forum_app).
    if bf_db::is_superuser(&pool).await.unwrap_or(false) {
        tracing::error!(
            "BF_DATABASE_URL connects as a Postgres SUPERUSER. Run the server as the least-privilege \
             forum_app role (deploy/postgres/roles.sql); a superuser connection is a remote-code-execution risk."
        );
    }
    let state = AppState::new(config.clone(), pool).await?;
    state.spawn_background_tasks();

    let stopping = state.clone();
    let service = app(state).into_make_service_with_connect_info::<SocketAddr>();

    // Terminate TLS here when a certificate is configured, otherwise serve plain HTTP and
    // leave TLS to the reverse proxy in front.
    if let (Some(cert), Some(key)) = (config.tls_cert.clone(), config.tls_key.clone()) {
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
            .await
            .map_err(|e| format!("TLS certificate or key could not be loaded: {e}"))?;
        spawn_http_redirector(&config);
        tracing::info!(addr = %config.bind, "listening with TLS");
        let handle = axum_server::Handle::new();
        let stopper = handle.clone();
        tokio::spawn(async move {
            bf_server::http::observability::shutdown_signal().await;
            stopper.graceful_shutdown(Some(std::time::Duration::from_secs(25)));
        });
        axum_server::bind_rustls(config.bind, tls).handle(handle).serve(service).await?;
        bf_server::platform::ads::flush(&stopping).await;
        return Ok(());
    }

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(addr = %config.bind, "listening");
    axum::serve(listener, service).with_graceful_shutdown(bf_server::http::observability::shutdown_signal()).await?;
    // Impressions counted since the last tick would otherwise be lost with the process.
    bf_server::platform::ads::flush(&stopping).await;
    Ok(())
}

/// With `BF_HTTP_BIND` set (typically `0.0.0.0:80`), answers every plain-HTTP request with a
/// redirect to the canonical HTTPS URL. A separate listener rather than a branch in the main
/// router, so the application itself is only ever reached over TLS.
fn spawn_http_redirector(config: &Config) {
    let Some(addr) = std::env::var("BF_HTTP_BIND").ok().filter(|v| !v.is_empty()) else { return };
    let addr: SocketAddr = if let Ok(a) = addr.parse() {
        a
    } else {
        tracing::error!(%addr, "BF_HTTP_BIND is not an address; the HTTP redirector is off");
        return;
    };
    let base = config.base_url.clone();
    tokio::spawn(async move {
        let redirect = move |uri: axum::http::Uri| {
            let base = base.clone();
            async move {
                let path = uri.path_and_query().map_or("/", |p| p.as_str());
                axum::response::Redirect::permanent(&format!("{base}{path}"))
            }
        };
        let router: axum::Router = axum::Router::new().fallback(axum::routing::any(redirect));
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                tracing::info!(%addr, "redirecting HTTP to HTTPS");
                if let Err(e) = axum::serve(listener, router).await {
                    tracing::error!(error = %e, "HTTP redirector stopped");
                }
            }
            Err(e) => tracing::error!(error = %e, %addr, "HTTP redirector could not bind"),
        }
    });
}
