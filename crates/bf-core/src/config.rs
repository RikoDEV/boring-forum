//! Process configuration, read once from environment variables (`BF_*`).

use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    /// Max DB connections; defaults to the CPU count (pool sized to cores).
    pub db_pool_size: u32,
    /// Lifetime of a guest microcache entry, in milliseconds (1–5 s).
    pub microcache_ttl_ms: u64,
    /// Requests per minute allowed per client IP.
    pub rate_limit_per_min: u32,
    pub log_json: bool,
    /// Bearer token for `/api/v1/admin/*` until accounts and staff groups exist.
    /// When unset, admin endpoints are disabled.
    pub admin_token: Option<String>,
    /// Directory holding the built admin SPA (`web/admin/build`).
    pub admin_dir: std::path::PathBuf,
    /// Public origin used in sitemaps, feeds and canonical URLs, e.g. `https://forum.example`.
    pub base_url: String,
    /// PEM certificate chain and private key. When both are set the binary terminates TLS
    /// itself with rustls, which allows TLS 1.2 and 1.3 only.
    pub tls_cert: Option<std::path::PathBuf>,
    pub tls_key: Option<std::path::PathBuf>,
    /// Redirect plain HTTP to HTTPS. See [`HttpsRedirect`].
    pub https_redirect: HttpsRedirect,
    /// Add `preload` to the HSTS header. Opt-in: submitting a domain to the preload list is
    /// effectively permanent, so it must be a deliberate choice.
    pub hsts_preload: bool,
}

/// When to answer a plain-HTTP request with a redirect to HTTPS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpsRedirect {
    /// Never, for local development and for plain-HTTP deployments.
    Off,
    /// Only when a proxy tells us the request arrived over HTTP (`X-Forwarded-Proto: http`).
    /// This is the default for an `https://` base URL: a proxy that forwards no protocol header
    /// gets no redirect, so a misconfigured one cannot produce a redirect loop. The header is
    /// believed only from a peer listed in `BF_TRUSTED_PROXIES` (as with `X-Forwarded-For`); the
    /// proxy must be listed there or this mode never redirects.
    Proxied,
    /// Every request that did not arrive over TLS at this process. Correct when the binary
    /// terminates TLS itself or sits behind a proxy that always sets the header. The header is
    /// believed only from a peer listed in `BF_TRUSTED_PROXIES`; any other peer is treated as
    /// plain HTTP whatever it sends.
    Always,
}

impl Config {
    /// Settings that load but are probably a mistake, for startup logs and `--check-config`.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.base_url.starts_with("http://")
            && self.https_redirect == HttpsRedirect::Off
            && !self.base_url.contains("127.0.0.1")
            && !self.base_url.contains("localhost")
        {
            out.push(format!(
                "BF_BASE_URL {} is plain http and BF_HTTPS_REDIRECT is off: sessions and passwords cross the network in clear text",
                self.base_url
            ));
        }
        out
    }

    /// A production site (https) without `BF_SECRET_KEY` would keep TOTP and webhook secrets in
    /// plain text and sign links with a key that changes at every start: refused, unless
    /// `BF_ALLOW_NO_SECRET_KEY=1`. A plain-http address (local development) is left alone.
    pub fn secret_key_error(&self, key_set: bool) -> Option<String> {
        let allowed = std::env::var("BF_ALLOW_NO_SECRET_KEY").is_ok_and(|v| v.trim() == "1");
        (self.base_url.starts_with("https://") && !key_set && !allowed).then(|| {
            "BF_SECRET_KEY is not set on an https site: secrets would be stored in plain text.              Make one with `boring-forum secret-key`, or set BF_ALLOW_NO_SECRET_KEY=1 to run without"
                .to_owned()
        })
    }

    pub fn from_env() -> Result<Self, String> {
        fn var(key: &str) -> Option<String> {
            std::env::var(key).ok().filter(|v| !v.is_empty())
        }
        fn parse<T: std::str::FromStr>(key: &str, default: T) -> Result<T, String> {
            match var(key) {
                Some(v) => v.parse().map_err(|_| format!("{key}: invalid value {v:?}")),
                None => Ok(default),
            }
        }
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get() as u32);
        let bind: SocketAddr = parse("BF_BIND", SocketAddr::from(([127, 0, 0, 1], 8080)))?;
        let base_url = var("BF_BASE_URL").unwrap_or_else(|| format!("http://{bind}")).trim_end_matches('/').to_owned();
        let tls_cert = var("BF_TLS_CERT").map(Into::into);
        let tls_key = var("BF_TLS_KEY").map(Into::into);
        if tls_cert.is_some() != tls_key.is_some() {
            return Err("BF_TLS_CERT and BF_TLS_KEY must be set together".into());
        }
        let https_redirect = match var("BF_HTTPS_REDIRECT").as_deref() {
            Some("off" | "false") => HttpsRedirect::Off,
            Some("proxied") => HttpsRedirect::Proxied,
            Some("always" | "true") => HttpsRedirect::Always,
            Some(other) => return Err(format!("BF_HTTPS_REDIRECT: expected off, proxied or always, got {other:?}")),
            // Terminating TLS here means every plain request is genuinely plain, so redirect it.
            None if tls_cert.is_some() => HttpsRedirect::Always,
            None if base_url.starts_with("https://") => HttpsRedirect::Proxied,
            None => HttpsRedirect::Off,
        };
        Ok(Self {
            base_url,
            bind,
            tls_cert,
            tls_key,
            https_redirect,
            hsts_preload: parse("BF_HSTS_PRELOAD", false)?,
            database_url: var("BF_DATABASE_URL").or_else(|| var("DATABASE_URL")).ok_or("BF_DATABASE_URL is not set")?,
            db_pool_size: parse("BF_DB_POOL_SIZE", cores.max(2))?,
            microcache_ttl_ms: parse("BF_MICROCACHE_TTL_MS", 2000)?,
            rate_limit_per_min: parse("BF_RATE_LIMIT_PER_MIN", 600)?,
            log_json: parse("BF_LOG_JSON", false)?,
            // A short token used to be dropped without a word, leaving the admin API off while
            // the operator believed it was on (or, worse, expected a weak token to be refused).
            admin_token: match var("BF_ADMIN_TOKEN") {
                Some(t) if t.len() < 32 => {
                    return Err(
                        "BF_ADMIN_TOKEN is set but shorter than 32 characters; use a longer one or unset it".into()
                    );
                }
                other => other,
            },
            admin_dir: var("BF_ADMIN_DIR").unwrap_or_else(|| "web/admin/build".into()).into(),
        })
    }
}
