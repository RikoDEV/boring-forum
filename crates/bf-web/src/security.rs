//! The page's inline code and the Content-Security-Policy that allows exactly that code.
//!
//! Scripts are allowed by hash rather than nonce: guest pages come from a shared microcache,
//! and a cached nonce would be no protection. The inline scripts contain no request data.

use crate::assets::{ISLANDS, MEMBERS};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

/// Applies the stored theme before the first paint, to the page's CSS and to its `color-scheme`
/// meta tag. Byte for byte what `layout.html` contains: the policy allows it by hash.
pub const THEME_SCRIPT: &str = r#"try{var t=localStorage.getItem("bf-theme");if(t){document.documentElement.dataset.theme=t;document.querySelector('meta[name="color-scheme"]').content=t}}catch(e){}"#;
pub const SPECULATION_RULES: &str = r#"{"prerender":[{"where":{"and":[{"href_matches":"/*"},{"not":{"href_matches":["/logout","/t/*","/notifications/*","/language/*","/safe-mode","/c/*/go"]}}]},"eagerness":"moderate"}]}"#;

static LOADER: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"const els = document.querySelectorAll("[data-island]");
if (els.length) import("{}").then(m => {{
  const rest = [...els].filter(el => !m.mount(el.dataset.island, el));
  if (rest.length) import("{}").then(mm => rest.forEach(el => mm.mount(el.dataset.island, el)));
}});"#,
        ISLANDS.url(),
        MEMBERS.url()
    )
});

pub fn loader_script() -> String {
    LOADER.clone()
}

pub fn critical_css() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/critical.css"))
}

/// The CSP source that allows exactly this inline code: `'sha256-…'`.
pub fn hash(code: &str) -> String {
    use base64::Engine;
    format!("'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(code.as_bytes())))
}

/// Script hosts of the optional registration challenge, for the policy and the page.
pub fn captcha_origin(provider: &str) -> Option<&'static str> {
    match provider {
        "turnstile" => Some("https://challenges.cloudflare.com"),
        "recaptcha" => Some("https://www.google.com/recaptcha/ https://www.gstatic.com/recaptcha/"),
        "hcaptcha" => Some("https://js.hcaptcha.com https://hcaptcha.com https://*.hcaptcha.com"),
        _ => None,
    }
}

/// Image sources: any https host, or only those in the `csp_image_hosts` setting. Entries
/// that are not plain https origins are ignored, so a typo cannot alter the policy.
fn image_sources(hosts: &str) -> String {
    let valid: Vec<&str> = hosts
        .split_whitespace()
        .filter(|h| {
            h.strip_prefix("https://").is_some_and(|r| {
                !r.is_empty() && r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '*'))
            })
        })
        .collect();
    if valid.is_empty() { "https:".to_owned() } else { format!("https://i.ytimg.com {}", valid.join(" ")) }
}

/// The GIF provider a setting names (`klipy` or `giphy`), or None for anything else.
pub fn gif_provider(name: &str) -> Option<&'static str> {
    match name {
        "klipy" => Some("klipy"),
        "giphy" => Some("giphy"),
        _ => None,
    }
}

/// A provider's API origin and the hosts it serves pictures from, for the policy.
fn gif_hosts(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "klipy" => Some((
            "https://api.klipy.com",
            "https://static.klipy.com https://static1.klipy.com https://static2.klipy.com",
        )),
        "giphy" => Some(("https://api.giphy.com", "https://*.giphy.com")),
        _ => None,
    }
}

/// Web analytics. Plausible and Umami set no cookies and load at once; GA4 and Matomo
/// set cookies, so they wait for consent when a cookie notice is configured, and they load more
/// scripts, which Trusted Types would block. Settings that do not look right load nothing.
#[derive(Debug, Clone, Default)]
pub struct Analytics {
    pub provider: &'static str,
    pub id: String,
    /// `https://host[:port]`, without a path.
    pub origin: String,
}

fn https_origin(raw: &str) -> Option<String> {
    let rest = raw.trim().trim_end_matches('/').strip_prefix("https://")?;
    let ok = !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    ok.then(|| format!("https://{}", rest.to_ascii_lowercase()))
}

impl Analytics {
    pub fn from_settings(s: &bf_core::settings::Settings) -> Self {
        let id = s.str("analytics_id").trim().to_owned();
        let host = s.str("analytics_host");
        let ok_id = |f: fn(char) -> bool| !id.is_empty() && id.len() <= 100 && id.chars().all(f);
        let (provider, origin) = match s.str("analytics_provider") {
            "plausible" if ok_id(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ',')) => {
                ("plausible", https_origin(host).unwrap_or_else(|| "https://plausible.io".into()))
            }
            "umami" if ok_id(|c| c.is_ascii_hexdigit() || c == '-') => match https_origin(host) {
                Some(o) => ("umami", o),
                None => return Self::default(),
            },
            "ga4" if id.starts_with("G-") && ok_id(|c| c.is_ascii_alphanumeric() || c == '-') => {
                ("ga4", "https://www.googletagmanager.com".into())
            }
            "matomo" if ok_id(|c| c.is_ascii_digit()) => match https_origin(host) {
                Some(o) => ("matomo", o),
                None => return Self::default(),
            },
            _ => return Self::default(),
        };
        Self { provider, id, origin }
    }

    /// Needs consent and loads scripts of its own.
    pub fn gated(&self) -> bool {
        matches!(self.provider, "ga4" | "matomo")
    }

    fn script_src(&self) -> String {
        match self.provider {
            "" => String::new(),
            "ga4" => "https://www.googletagmanager.com".into(),
            _ => self.origin.clone(),
        }
    }

    fn connect_src(&self) -> String {
        match self.provider {
            "" => String::new(),
            "ga4" => {
                "https://*.google-analytics.com https://*.analytics.google.com https://www.googletagmanager.com".into()
            }
            _ => self.origin.clone(),
        }
    }

    /// The tag for `<head>`: the cookieless providers' script, or nothing.
    pub fn head(&self) -> String {
        match self.provider {
            "plausible" => {
                format!(
                    "<script defer data-domain=\"{}\" src=\"{}/js/script.js\"></script>",
                    bf_core::html::esc(&self.id),
                    bf_core::html::esc(&self.origin)
                )
            }
            "umami" => {
                format!(
                    "<script defer data-website-id=\"{}\" src=\"{}/script.js\"></script>",
                    bf_core::html::esc(&self.id),
                    bf_core::html::esc(&self.origin)
                )
            }
            _ => String::new(),
        }
    }

    /// For the consent-gated providers: the island that loads them once allowed.
    pub fn island(&self) -> String {
        if !self.gated() {
            return String::new();
        }
        let props = serde_json::json!({ "provider": self.provider, "id": self.id, "origin": self.origin }).to_string();
        format!("<span data-island=\"analytics\" data-props=\"{}\" hidden></span>", bf_core::html::esc(&props))
    }
}

static PLUGIN_CONNECT: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

/// Origins the approved plugins' browser code may contact (their `connect:` capabilities),
/// added to `connect-src`. Set by the plugin host whenever plugins are loaded.
pub fn set_plugin_connect(origins: Vec<String>) {
    *PLUGIN_CONNECT.write().unwrap_or_else(std::sync::PoisonError::into_inner) = origins;
}

///. Inline CSS and scripts are allowed by hash only.
/// `custom` is the admin's custom code; `None` in safe mode, where pages leave it out.
pub fn content_security_policy(
    captcha: &str,
    image_hosts: &str,
    analytics: &Analytics,
    custom: Option<&crate::custom_code::CustomCode>,
    gifs: Option<&str>,
    imgproxy: Option<&str>,
) -> String {
    let extra = captcha_origin(captcha).unwrap_or("");
    let (mut scripts, mut connect) = (analytics.script_src(), analytics.connect_src());
    // The GIF picker asks its provider's API from the browser; the pictures load from the provider's hosts.
    let mut images = image_sources(image_hosts);
    // Pictures from other sites arrive through imgproxy: with pictures limited to listed hosts, it is one.
    if let Some(origin) = imgproxy.filter(|o| {
        o.starts_with("http") && o.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b';' | b',' | b'\'' | b'"'))
    }) && images != "https:"
    {
        images.push(' ');
        images.push_str(origin);
    }
    if let Some((api, media)) = gifs.and_then(gif_hosts) {
        connect.push(' ');
        connect.push_str(api);
        if images != "https:" {
            images.push(' ');
            images.push_str(media);
        }
    }
    for origin in PLUGIN_CONNECT.read().unwrap_or_else(std::sync::PoisonError::into_inner).iter() {
        connect.push(' ');
        connect.push_str(origin);
    }
    let (mut styles, mut frames) = (String::new(), String::new());
    if let Some(c) = custom {
        let lists = [
            (&mut scripts, &c.script_src),
            (&mut styles, &c.style_src),
            (&mut connect, &c.connect_src),
            (&mut frames, &c.connect_src),
        ];
        for (list, add) in lists {
            for s in add {
                list.push(' ');
                list.push_str(s);
            }
        }
    }
    // The providers' scripts load more scripts without Trusted Types, so a page with one cannot
    // enforce them; nor can a page with scripts from the custom code.
    let trusted = if extra.is_empty() && !analytics.gated() && !custom.is_some_and(|c| c.scripts) {
        "require-trusted-types-for 'script'; trusted-types svelte-trusted-html bf-plugin bf-ai bf-cooked bf-preview bf-page bf-live bf-terms bf-quick-edit bf-worker; "
    } else {
        ""
    };
    // Third-party scripts (a consent banner, an ad widget) write `style=""` attributes and add
    // `<style>` elements at run time, which no hash can list. They may already run any script from
    // their hosts, so the page's own styles are the only thing left to protect: elements keep the
    // site's sources, and without the hashes, which would cancel `'unsafe-inline'`.
    let style_attr = if custom.is_some_and(|c| c.scripts) {
        let hosts: String = custom
            .map(|c| c.style_src.iter().filter(|s| !s.starts_with('\'')).map(|s| format!(" {s}")).collect())
            .unwrap_or_default();
        format!("style-src-attr 'unsafe-inline'; style-src-elem 'self' 'unsafe-inline' {extra}{hosts}; ")
    } else {
        String::new()
    };
    format!(
        "default-src 'self'; script-src 'self' {} {} {} {extra} {scripts}; style-src 'self' {} 'unsafe-hashes' {} {} {} {extra}{styles}; \
         {style_attr}img-src 'self' data: {}; media-src 'self'; connect-src 'self' {extra} {connect}; frame-src 'self' {extra}{frames}; \
         object-src 'none'; base-uri 'none'; frame-ancestors 'self'; form-action 'self'; \
         {trusted}\
         report-uri /api/v1/csp-report; report-to csp",
        hash(THEME_SCRIPT),
        hash(SPECULATION_RULES),
        hash(&LOADER),
        hash(critical_css()),
        // The only style attributes allowed, exactly as CodeMirror writes them.
        hash("tab-size: 4;"),
        hash("-moz-tab-size: 4;"),
        hash("pointer-events: none;"),
        images,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analytics_hosts_are_plain_origins() {
        assert_eq!(https_origin("https://stats.example.com/"), Some("https://stats.example.com".into()));
        assert_eq!(https_origin("https://a.example; script-src *"), None);
        assert_eq!(https_origin("http://a.example"), None);
    }

    #[test]
    fn image_hosts_cannot_alter_the_policy() {
        assert_eq!(image_sources(""), "https:");
        assert_eq!(image_sources("https://a.example; script-src *"), "https:");
        assert_eq!(image_sources("https://a.example http://b.example"), "https://i.ytimg.com https://a.example");
    }

    #[test]
    fn a_gif_provider_opens_only_its_own_hosts() {
        let policy = |hosts: &str, gifs| content_security_policy("", hosts, &Analytics::default(), None, gifs, None);
        let off = policy("", None);
        assert!(!off.contains("klipy") && !off.contains("giphy"));
        let klipy = policy("", Some("klipy"));
        assert!(klipy.contains("connect-src 'self'") && klipy.contains("https://api.klipy.com"));
        // With every https host already allowed for pictures, only the API needs opening.
        assert!(!klipy.contains("static.klipy.com"));
        assert!(policy("https://cdn.example", Some("giphy")).contains("https://*.giphy.com"));
        assert!(policy("https://cdn.example", Some("klipy")).contains("https://static.klipy.com"));
        assert!(!policy("", Some("nonsense")).contains("klipy") && gif_provider("nonsense").is_none());
    }

    #[test]
    fn custom_scripts_may_set_style_attributes_but_nothing_else_inline() {
        let policy = |custom: Option<&crate::custom_code::CustomCode>| {
            content_security_policy("", "", &Analytics::default(), custom, None, None)
        };
        assert!(!policy(None).contains("style-src-attr"));
        let plain = crate::custom_code::CustomCode::default();
        assert!(!policy(Some(&plain)).contains("style-src-attr"));
        let scripted = crate::custom_code::CustomCode { scripts: true, ..Default::default() };
        let csp = policy(Some(&scripted));
        assert!(csp.contains("style-src-attr 'unsafe-inline';"));
        // Elements added at run time are allowed too, from the site's own sources only.
        let elem = csp.split("; ").find(|d| d.starts_with("style-src-elem ")).unwrap();
        assert!(elem.contains("'unsafe-inline'") && !elem.contains("sha256-"));
        // Pictures inlined as `data:` URLs (a consent banner's icons) load.
        assert!(csp.contains("img-src 'self' data:"));
        // The `style-src` list, which the page's own hashed styles use, still has no `unsafe-inline`.
        let style_src = csp.split("; ").find(|d| d.starts_with("style-src ")).unwrap();
        assert!(!style_src.contains("'unsafe-inline'"));
    }

    #[test]
    fn imgproxy_is_a_picture_source_only_when_pictures_are_limited() {
        let policy = |hosts: &str, origin: Option<&str>| {
            content_security_policy("", hosts, &Analytics::default(), None, None, origin)
        };
        assert!(
            !policy("", Some("https://img.example.com")).contains("img.example.com"),
            "every https host is already allowed"
        );
        assert!(
            policy("https://cdn.example", Some("https://img.example.com"))
                .contains("https://cdn.example https://img.example.com")
        );
        assert!(policy("https://cdn.example", Some("https://i.example; script-src *")).contains("script-src 'self'"));
        assert!(!policy("https://cdn.example", Some("https://i.example; script-src *")).contains("i.example"));
    }
}
