//! Content apps: pages a plugin serves under `/app/<plugin>/…`. A plugin declares
//! `[[routes]]` (paths with parameters, GET and POST, a feed); the host does everything around
//! the plugin's answer: it checks who may open the route, the CSRF token and the rate of
//! posting before the plugin runs, then sanitizes what it answers and puts it in the site's
//! shell with the title, description, canonical address, Open Graph and JSON-LD it asked for.

use crate::http::ui::{Shell, begin, finish, panel};
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_core::i18n::en;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::extras::strings_for;
use super::host::local_path;
use super::{call, listening};

const MAX_ROUTES: usize = 40;
const MAX_SEGMENTS: usize = 8;
const DEFAULT_BODY: usize = 32 * 1024;
const MAX_BODY: usize = 256 * 1024;
const MAX_FIELDS: usize = 100;
const MAX_FIELD_CHARS: usize = 20_000;
const MAX_FEED_ITEMS: usize = 100;
const MAX_JSON_LD: usize = 3;
const MAX_JSON_LD_BYTES: usize = 20_000;

fn get_only() -> Vec<String> {
    vec!["GET".into()]
}

/// `[[routes]]`: one address of the plugin's app.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct RouteDecl {
    /// Names the route in `page.request` (`route`).
    pub(super) key: String,
    /// Under `/app/<plugin>/`: literal segments and `{param}` ones, `entries/{slug}`. Empty is the
    /// app's front page.
    pub(super) path: String,
    #[serde(default = "get_only")]
    pub(super) methods: Vec<String>,
    /// `page` (the default) or `feed` (the plugin answers a list; the host writes RSS, Atom and JSON Feed).
    #[serde(default)]
    pub(super) kind: String,
    /// Only signed-in members open it. Always so for a route that takes POST.
    #[serde(default)]
    pub(super) login: bool,
    /// A key of the plugin's `[[permissions]]`: only members whose group holds it (and admins).
    #[serde(default)]
    pub(super) permission: Option<String>,
    /// Largest POST body in bytes (default 32 KB, at most 256 KB).
    #[serde(default)]
    pub(super) max_body: Option<usize>,
}

#[derive(Debug, PartialEq)]
enum Seg {
    Lit(String),
    Param(String),
}

fn segments(path: &str) -> Result<Vec<Seg>, String> {
    if path.is_empty() {
        return Ok(vec![]);
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > MAX_SEGMENTS {
        return Err(format!("Route {path} has too many segments."));
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for part in parts {
        if let Some(name) = part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
            let ok = !name.is_empty()
                && name.len() <= 30
                && name.starts_with(|c: char| c.is_ascii_lowercase())
                && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !ok || seen.contains(&name) {
                return Err(format!("Bad parameter in route {path}."));
            }
            seen.push(name);
            out.push(Seg::Param(name.to_owned()));
        } else {
            let ok = !part.is_empty()
                && part.len() <= 60
                && part != "."
                && part != ".."
                && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
            if !ok {
                return Err(format!("Bad segment in route {path}: use lowercase letters, digits, dashes, dots and {{param}}."));
            }
            out.push(Seg::Lit(part.to_owned()));
        }
    }
    Ok(out)
}

/// Checks `[[routes]]` when the package is read. `permissions` are the plugin's own `[[permissions]]` keys.
pub(super) fn validate_routes(routes: &[RouteDecl], capabilities: &[String], permissions: &[String]) -> Result<(), String> {
    if routes.is_empty() {
        return Ok(());
    }
    if !capabilities.iter().any(|c| c == "pages") {
        return Err(en("A plugin that declares routes must ask for the pages capability.").into());
    }
    if routes.len() > MAX_ROUTES {
        return Err(format!("A plugin may declare {MAX_ROUTES} routes at most."));
    }
    let mut shapes: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for (i, r) in routes.iter().enumerate() {
        if !super::registry::good_key(&r.key) || routes[..i].iter().any(|o| o.key == r.key) {
            return Err(format!("Bad or repeated route key {}.", r.key));
        }
        let segs = segments(&r.path)?;
        if r.methods.is_empty() || r.methods.iter().any(|m| m != "GET" && m != "POST") {
            return Err(format!("Route {} takes GET and POST only.", r.key));
        }
        if !matches!(r.kind.as_str(), "" | "page" | "feed") {
            return Err(format!("Route {} has an unknown kind.", r.key));
        }
        if r.max_body.is_some_and(|b| b == 0 || b > MAX_BODY) {
            return Err(format!("Route {}: max_body is 1 to {MAX_BODY} bytes.", r.key));
        }
        if let Some(p) = &r.permission
            && !permissions.contains(p)
        {
            return Err(format!("Route {} needs permission {p}, which the plugin does not declare.", r.key));
        }
        if r.kind == "feed" {
            let last_is_rss = matches!(segs.last(), Some(Seg::Lit(l)) if l.ends_with(".rss"));
            if r.methods != ["GET"] || r.login || r.permission.is_some() || !last_is_rss {
                return Err(format!("Feed route {} must be a public GET route ending in .rss.", r.key));
            }
        }
        let shape: Vec<String> = segs
            .iter()
            .map(|s| match s {
                Seg::Lit(l) => l.clone(),
                Seg::Param(_) => "{}".to_owned(),
            })
            .collect();
        if shapes.iter().any(|(s, m)| *s == shape && m.iter().any(|x| r.methods.contains(x))) {
            return Err(format!("Route {} repeats the address of another route.", r.key));
        }
        shapes.push((shape, r.methods.clone()));
    }
    Ok(())
}

/// The format a feed address asks for: the route is declared as `….rss`, and `.atom` and `.json`
/// beside it are the same feed.
#[derive(Clone, Copy, PartialEq, Debug)]
enum FeedFormat {
    Rss,
    Atom,
    Json,
}

enum Hit<'a> {
    Found { route: &'a RouteDecl, params: Map<String, Value>, format: FeedFormat },
    Method(Vec<String>),
    None,
}

fn match_route<'a>(routes: &'a [RouteDecl], rest: &str, method: &str) -> Hit<'a> {
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let mut path = rest.to_owned();
    let mut format = FeedFormat::Rss;
    for (ext, f) in [(".atom", FeedFormat::Atom), (".json", FeedFormat::Json)] {
        if let Some(stem) = rest.strip_suffix(ext) {
            path = format!("{stem}.rss");
            format = f;
        }
    }
    let given: Vec<&str> = if path.is_empty() { vec![] } else { path.split('/').collect() };
    let mut best: Option<(usize, &RouteDecl, Map<String, Value>)> = None;
    let mut other_methods: Vec<String> = Vec::new();
    for r in routes {
        // `.atom` and `.json` belong to feeds only.
        if format != FeedFormat::Rss && r.kind != "feed" {
            continue;
        }
        let Ok(segs) = segments(&r.path) else { continue };
        if segs.len() != given.len() {
            continue;
        }
        let mut params = Map::new();
        let mut literals = 0;
        let fits = segs.iter().zip(&given).all(|(s, g)| match s {
            Seg::Lit(l) => {
                literals += 1;
                l == g
            }
            Seg::Param(name) => {
                let ok = !g.is_empty() && g.chars().count() <= 200 && !g.chars().any(char::is_control);
                if ok {
                    params.insert(name.clone(), json!(g));
                }
                ok
            }
        });
        if !fits {
            continue;
        }
        if !r.methods.iter().any(|m| m == method) {
            other_methods.extend(r.methods.iter().cloned());
            continue;
        }
        if best.as_ref().is_none_or(|(n, ..)| literals > *n) {
            best = Some((literals, r, params));
        }
    }
    match best {
        Some((_, route, params)) => Hit::Found { route, params, format },
        None if !other_methods.is_empty() => Hit::Method(other_methods),
        None => Hit::None,
    }
}

/// The plugin's permission keys this viewer holds (administrators hold all).
async fn held_permissions(state: &AppState, plugin: &str, v: &Viewer) -> Vec<String> {
    let (Some(pool), Some(_)) = (state.pool.as_ref(), v.user.as_ref()) else { return vec![] };
    let Ok(all) = bf_db::platform::registry_groups::permissions_of(pool, plugin).await else { return vec![] };
    let mine: Vec<i64> = v.actor.groups.iter().map(|g| g.get()).collect();
    all.into_iter().filter(|(.., groups)| v.actor.is_admin || groups.iter().any(|g| mine.contains(g))).map(|(k, ..)| k).collect()
}

fn pairs(raw: &[u8], cap: usize) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in url::form_urlencoded::parse(raw).take(cap) {
        let v: String = v.chars().take(MAX_FIELD_CHARS).collect();
        let k: String = k.chars().take(60).collect();
        match out.get_mut(&k) {
            // A repeated name (checkboxes, a multiple select) becomes a list.
            Some(Value::Array(list)) if list.len() < 50 => list.push(json!(v)),
            Some(old) if !old.is_array() => *old = json!([old.clone(), v]),
            Some(_) => {}
            None => {
                out.insert(k, json!(v));
            }
        }
    }
    out
}

pub(super) async fn app_root(
    State(state): State<AppState>,
    v: Viewer,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    serve(&state, v, method, headers, uri.query().unwrap_or_default().to_owned(), id, String::new(), body).await
}

pub(super) async fn app_path(
    State(state): State<AppState>,
    v: Viewer,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    Path((id, rest)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    serve(&state, v, method, headers, uri.query().unwrap_or_default().to_owned(), id, rest, body).await
}

#[expect(clippy::too_many_arguments, reason = "one request, taken apart by the two entry points above")]
async fn serve(
    state: &AppState,
    v: Viewer,
    method: Method,
    headers: HeaderMap,
    query: String,
    id: String,
    rest: String,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    let p = listening("page.request")
        .into_iter()
        .find(|p| p.id == id && p.capabilities.iter().any(|c| c == "pages"))
        .ok_or(AppError::NotFound)?;
    let verb = if method == Method::HEAD { Method::GET } else { method.clone() };
    let (route, params, format) = match match_route(&p.routes, &rest, verb.as_str()) {
        Hit::Found { route, params, format } => (route.clone(), params, format),
        Hit::Method(allowed) => {
            let mut allowed = allowed;
            allowed.sort();
            allowed.dedup();
            return Ok((StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, allowed.join(", "))]).into_response());
        }
        Hit::None => return Err(AppError::NotFound),
    };
    let posting = verb == Method::POST;
    // Who may open it is decided here, before the plugin sees the request.
    let held = held_permissions(state, &id, &v).await;
    if route.login || posting || route.permission.is_some() {
        v.require_user()?;
        if route.permission.as_ref().is_some_and(|k| !held.contains(k)) {
            return Err(AppError::Forbidden);
        }
    }
    let mut form = Map::new();
    if posting {
        let limit = route.max_body.unwrap_or(DEFAULT_BODY).min(MAX_BODY);
        if body.len() > limit {
            return Err(AppError::Validation("That form is too large.".into()));
        }
        let ct = headers.get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).unwrap_or_default();
        if !ct.starts_with("application/x-www-form-urlencoded") {
            return Err(AppError::BadRequest("Send the form as application/x-www-form-urlencoded.".into()));
        }
        form = pairs(&body, MAX_FIELDS);
        let header_token = crate::http::viewer::csrf_header(&headers).to_owned();
        let token = form.remove("_csrf").and_then(|t| t.as_str().map(str::to_owned)).unwrap_or(header_token);
        v.check_csrf(&token)?;
        let user = v.require_user()?;
        if !super::routes::press_allowed(&id, user.id.get(), state.settings().int("plugin_actions_per_minute")) {
            return Err(AppError::RateLimited { retry_after_secs: 30 });
        }
    }
    let t = v.t();
    let data = json!({
        "route": route.key,
        "method": verb.as_str(),
        "path": if rest.is_empty() { format!("/app/{id}") } else { format!("/app/{id}/{rest}") },
        "params": params,
        "query": pairs(query.as_bytes(), 50),
        "form": form,
        "viewer_id": v.user.as_ref().map(|u| u.id.get()),
        "viewer": v.user.as_ref().map(|u| json!({
            "id": u.id.get(), "username": u.username, "is_admin": v.actor.is_admin, "staff": is_staff(&v),
        })),
        "permissions": held,
        "locale": t.locale,
        "dir": t.dir(),
        "strings": strings_for(&id, t.locale),
        "base_url": state.config.base_url,
    });
    let out = call(state, p, "page.request", &data).await.ok_or_else(|| AppError::internal("plugin failed"))?;
    if posting {
        // Guests may be served pages from a short cache; what was just written must show.
        state.microcache.clear();
    }
    if route.kind == "feed" {
        return feed(state, &id, &rest, format, &out);
    }
    page(state, &v, &id, &rest, &out, method == Method::HEAD, route.login || route.permission.is_some() || posting)
}

fn text(out: &Value, key: &str, max: usize) -> Option<String> {
    out.get(key).and_then(Value::as_str).map(|s| s.chars().take(max).collect::<String>()).filter(|s| !s.trim().is_empty())
}

/// An address on this site, or an `https` one, for a picture.
fn picture(state: &AppState, raw: &str) -> Option<String> {
    if local_path(raw) {
        Some(format!("{}{raw}", state.config.base_url))
    } else {
        raw.starts_with("https://").then(|| raw.to_owned())
    }
}

fn page(state: &AppState, v: &Viewer, id: &str, rest: &str, out: &Value, head: bool, gated: bool) -> Result<Response, AppError> {
    if let Some(to) = out.get("redirect").and_then(Value::as_str) {
        if !local_path(to) {
            return Err(AppError::internal("plugin redirected off the site"));
        }
        return Ok(Redirect::to(to).into_response());
    }
    let status = out
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|s| u16::try_from(s).ok())
        .filter(|s| matches!(s, 200 | 400 | 403 | 404 | 410 | 422))
        .and_then(|s| StatusCode::from_u16(s).ok())
        .unwrap_or(StatusCode::OK);
    let own = format!("/app/{id}");
    let ctx = begin(state, v);
    let html = bf_web::theme::clean_page_html(&text(out, "html", 400_000).unwrap_or_default(), id);
    let html = bf_web::theme::add_csrf_fields(&html, &ctx.token);
    // A page that is not a plain 200, or that only a member opens, is none for search engines to list.
    let flag = |k: &str| out.get(k).and_then(Value::as_bool).unwrap_or(false);
    let noindex = status != StatusCode::OK || flag("noindex") || flag("private") || gated;
    let canonical = out
        .get("canonical")
        .and_then(Value::as_str)
        .filter(|c| local_path(c))
        .map_or_else(|| if rest.is_empty() { own.clone() } else { format!("{own}/{rest}") }, str::to_owned);
    let og = out.get("og").unwrap_or(&Value::Null);
    let mut json_ld = Vec::new();
    let docs = match out.get("jsonld") {
        Some(Value::Array(list)) => list.clone(),
        Some(one @ Value::Object(_)) => vec![one.clone()],
        _ => vec![],
    };
    for doc in docs.iter().filter(|d| d.is_object()).take(MAX_JSON_LD) {
        let script = bf_seo::json_ld_script(doc);
        if script.len() <= MAX_JSON_LD_BYTES {
            json_ld.push(script);
        }
    }
    let crumbs = out
        .get("breadcrumbs")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .take(5)
                .filter_map(|c| {
                    let (label, url) = (c.get(0)?.as_str()?, c.get(1)?.as_str()?);
                    local_path(url).then(|| (label.chars().take(100).collect::<String>(), url.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();
    let shell = Shell {
        title: text(out, "title", 200).unwrap_or_else(|| id.to_owned()),
        crumbs,
        canonical: Some(canonical),
        description: text(out, "description", 300),
        feed: out.get("feed").and_then(Value::as_str).filter(|f| local_path(f) && f.ends_with(".rss")).map(str::to_owned),
        noindex,
        json_ld,
        og_type: og.get("type").and_then(Value::as_str).filter(|t| matches!(*t, "article" | "website" | "profile")).unwrap_or_default().to_owned(),
        og_image: og.get("image").and_then(Value::as_str).and_then(|i| picture(state, i)),
        ..Shell::default()
    };
    let mut res = finish(state, v, ctx, shell, panel(&format!("<div class=\"bf-app\" data-plugin=\"{}\">{html}</div>", bf_core::html::esc(id))));
    *res.status_mut() = status;
    if head {
        *res.body_mut() = axum::body::Body::empty();
    }
    Ok(res)
}

/// The three feed formats from one answer: `{"feed": {title, description, link, items: [{title, link, html, published}]}}`.
/// Everything is escaped by the host, and item HTML is sanitized like any plugin output.
fn feed(state: &AppState, id: &str, rest: &str, format: FeedFormat, out: &Value) -> Result<Response, AppError> {
    let f = out.get("feed").ok_or_else(|| AppError::internal("plugin sent no feed"))?;
    let base = &state.config.base_url;
    let site = state.settings().str("site_name").to_owned();
    let abs = |p: &str| if local_path(p) { format!("{base}{p}") } else { format!("{base}/app/{id}") };
    let title = text(f, "title", 200).unwrap_or_else(|| site.clone());
    let link = abs(f.get("link").and_then(Value::as_str).unwrap_or_default());
    let description = text(f, "description", 500).unwrap_or_default();
    let items: Vec<bf_seo::FeedItem> = f
        .get("items")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .take(MAX_FEED_ITEMS)
                .filter_map(|i| {
                    Some(bf_seo::FeedItem {
                        title: text(i, "title", 200)?,
                        link: abs(i.get("link").and_then(Value::as_str)?),
                        description_html: bf_web::theme::clean_html(&text(i, "html", 20_000).unwrap_or_default()),
                        published: i
                            .get("published")
                            .and_then(Value::as_str)
                            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                            .map_or_else(chrono::Utc::now, |d| d.with_timezone(&chrono::Utc)),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let stem = rest.strip_suffix(".rss").or_else(|| rest.strip_suffix(".atom")).or_else(|| rest.strip_suffix(".json")).unwrap_or(rest);
    let self_link = |ext: &str| format!("{base}/app/{id}/{stem}.{ext}");
    let (kind, body) = match format {
        FeedFormat::Rss => ("application/rss+xml; charset=utf-8", bf_seo::rss(&title, &link, &description, &items)),
        FeedFormat::Atom => ("application/atom+xml; charset=utf-8", bf_seo::atom(&title, &link, &self_link("atom"), &site, &items)),
        FeedFormat::Json => (
            "application/feed+json; charset=utf-8",
            bf_seo::json_feed(&title, &link, &self_link("json"), &description, &items).to_string(),
        ),
    };
    Ok(([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "public, max-age=300")], body).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(key: &str, path: &str, methods: &[&str]) -> RouteDecl {
        RouteDecl {
            key: key.into(),
            path: path.into(),
            methods: methods.iter().map(|m| (*m).to_owned()).collect(),
            kind: String::new(),
            login: false,
            permission: None,
            max_body: None,
        }
    }

    fn caps() -> Vec<String> {
        vec!["pages".into()]
    }

    #[test]
    fn routes_match_by_shape_and_prefer_literals() {
        let routes = vec![
            route("entry", "entries/{slug}", &["GET"]),
            route("new", "entries/new", &["GET", "POST"]),
            route("home", "", &["GET"]),
        ];
        assert!(validate_routes(&routes, &caps(), &[]).is_ok());
        let Hit::Found { route, params, .. } = match_route(&routes, "entries/hello-world", "GET") else { panic!("no match") };
        assert_eq!((route.key.as_str(), params["slug"].as_str()), ("entry", Some("hello-world")));
        let Hit::Found { route, .. } = match_route(&routes, "entries/new", "POST") else { panic!("no match") };
        assert_eq!(route.key, "new", "a literal beats a parameter");
        assert!(matches!(match_route(&routes, "", "GET"), Hit::Found { .. }));
        assert!(matches!(match_route(&routes, "entries/x/y", "GET"), Hit::None));
        assert!(matches!(match_route(&routes, "entries/hello", "POST"), Hit::Method(_)));
        assert!(matches!(match_route(&routes, "entries/hello/", "GET"), Hit::Found { .. }), "one trailing slash is fine");
    }

    #[test]
    fn bad_routes_are_refused() {
        let bad = |r: Vec<RouteDecl>| validate_routes(&r, &caps(), &["write".to_owned()]).is_err();
        assert!(bad(vec![route("a", "/x", &["GET"])]));
        assert!(bad(vec![route("a", "x/../y", &["GET"])]));
        assert!(bad(vec![route("a", "X", &["GET"])]));
        assert!(bad(vec![route("a", "{a}/{a}", &["GET"])]));
        assert!(bad(vec![route("a", "x", &["DELETE"])]));
        assert!(bad(vec![route("a", "x", &[])]));
        assert!(bad(vec![route("a", "x/{p}", &["GET"]), route("b", "x/{q}", &["GET"])]), "same shape twice");
        assert!(!bad(vec![route("a", "x/{p}", &["GET"]), route("b", "x/{q}", &["POST"])]));
        let mut gated = route("a", "x", &["GET"]);
        gated.permission = Some("nope".into());
        assert!(bad(vec![gated.clone()]));
        gated.permission = Some("write".into());
        assert!(!bad(vec![gated]));
        assert!(validate_routes(&[route("a", "x", &["GET"])], &[], &[]).is_err(), "needs the pages capability");
    }

    #[test]
    fn feeds_serve_three_formats() {
        let mut feed = route("feed", "feed.rss", &["GET"]);
        feed.kind = "feed".into();
        let routes = vec![feed, route("page", "page.atom", &["GET"])];
        assert!(validate_routes(&routes, &caps(), &[]).is_ok());
        let Hit::Found { format, .. } = match_route(&routes, "feed.atom", "GET") else { panic!("no match") };
        assert_eq!(format, FeedFormat::Atom);
        let Hit::Found { format, .. } = match_route(&routes, "feed.json", "GET") else { panic!("no match") };
        assert_eq!(format, FeedFormat::Json);
        assert!(matches!(match_route(&routes, "page.json", "GET"), Hit::None), "only feeds have siblings");
        let mut private = route("feed", "feed.rss", &["GET"]);
        private.kind = "feed".into();
        private.login = true;
        assert!(validate_routes(&[private], &caps(), &[]).is_err());
    }

    #[test]
    fn repeated_fields_become_lists() {
        let m = pairs(b"a=1&b=x&b=y&b=z&c=%3Cb%3E", 100);
        assert_eq!(m["a"], "1");
        assert_eq!(m["b"], json!(["x", "y", "z"]));
        assert_eq!(m["c"], "<b>");
    }
}
