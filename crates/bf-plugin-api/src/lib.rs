//! SDK for boring-forum WebAssembly plugins.
//!
//! A plugin is a `cdylib` built for `wasm32-unknown-unknown`. The forum calls it once per hook
//! (a fresh instance each time, with fuel, memory and time limits), handing it JSON and reading
//! JSON back. This crate hides the bytes: you write a function from [`Request`] to [`Reply`],
//! and [`export!`] turns it into the `alloc` and `handle` exports the host expects.
//!
//! ```ignore
//! use bf_plugin_api::{export, Reply, Request};
//!
//! fn on_hook(req: Request) -> Reply {
//!     match req.hook.as_str() {
//!         "post.before_create" => {
//!             let p: bf_plugin_api::hooks::PostBeforeCreate = req.payload().unwrap();
//!             if p.raw.contains("spam") { Reply::reject("No spam, please.") } else { Reply::none() }
//!         }
//!         _ => Reply::none(),
//!     }
//! }
//! export!(on_hook);
//! ```
//!
//! What a plugin may do is decided by the capabilities in its `plugin.toml` that an administrator
//! approved. A host call that was not approved returns [`Error::Denied`]; it never panics.
//! The full list of hooks, host calls and limits is in `docs/plugins/abi.md`.

pub mod host;
pub mod hooks;

pub use host::Error;
pub use serde_json::{Value, json};

use serde::Deserialize;
use serde::de::DeserializeOwned;

/// What the host passes to `handle`: the hook's name, its payload and the plugin's settings.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub hook: String,
    #[serde(default)]
    pub data: Value,
    /// The values an administrator saved for this plugin (see `settings.json`).
    #[serde(default)]
    pub settings: Value,
}

impl Request {
    /// The payload as one of the types in [`hooks`], or your own.
    pub fn payload<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.data.clone())
    }

    /// A text setting, or `default`.
    pub fn setting_str<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.settings.get(key).and_then(Value::as_str).unwrap_or(default)
    }

    pub fn setting_bool(&self, key: &str, default: bool) -> bool {
        self.settings.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    pub fn setting_i64(&self, key: &str, default: i64) -> i64 {
        self.settings.get(key).and_then(Value::as_i64).unwrap_or(default)
    }
}

/// What `handle` answers. The meaning depends on the hook; the constructors cover them.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply(pub Value);

impl Reply {
    /// Nothing to say: a filter changes nothing, an action is done.
    pub fn none() -> Self {
        Self(json!({}))
    }

    /// A filter refuses the content, with a message the member reads.
    pub fn reject(message: impl Into<String>) -> Self {
        Self(json!({ "reject": message.into() }))
    }

    /// A filter's changed fields: `post.before_create` takes `{"raw": "..."}`,
    /// `post.after_cook` takes `{"html": "..."}`.
    pub fn data(data: Value) -> Self {
        Self(json!({ "data": data }))
    }

    /// HTML for a slot, widget or panel. The host sanitizes it again, so scripts and styles in it
    /// are dropped; use `class` and the plugin's own style sheet for looks.
    pub fn html(html: impl Into<String>) -> Self {
        Self(json!({ "html": html.into() }))
    }

    /// An `api.request` answer: HTTP status and a JSON body.
    pub fn json(status: u16, body: Value) -> Self {
        Self(json!({ "status": status, "json": body }))
    }

    /// A page for a `[[routes]]` route: HTML for the body (sanitized by the host: forms that post back
    /// to `/app/<id>/…`, `data-app-*` and classes are kept, scripts and styles are not) and the
    /// `<title>`. Add the rest with [`Reply::with`]: `description`, `canonical` (a path), `noindex`,
    /// `og` (`{"type": "article", "image": "…"}`), `jsonld` (an object or a list), `breadcrumbs`
    /// (`[["Notes", "/app/notes"]]`), `feed` (the path of the plugin's `.rss` route), `status`
    /// (200, 400, 403, 404, 410 or 422).
    pub fn page(title: impl Into<String>, html: impl Into<String>) -> Self {
        Self(json!({ "title": title.into(), "html": html.into() }))
    }

    /// Sends the visitor to a path on this site (after a POST): `303 See Other`.
    pub fn redirect(path: impl Into<String>) -> Self {
        Self(json!({ "redirect": path.into() }))
    }

    /// A feed for a route of `kind = "feed"`: the host writes RSS, Atom and JSON Feed from it.
    /// Items are `{title, link (a path), html, published (RFC 3339)}`.
    pub fn feed(title: impl Into<String>, link: impl Into<String>, description: impl Into<String>, items: Vec<Value>) -> Self {
        Self(json!({ "feed": { "title": title.into(), "link": link.into(), "description": description.into(), "items": items } }))
    }

    /// Adds one field to an answer that is a JSON object.
    #[must_use]
    pub fn with(mut self, key: &str, value: Value) -> Self {
        if let Some(map) = self.0.as_object_mut() {
            map.insert(key.to_owned(), value);
        }
        self
    }

    /// A `toast` answer for `action.run`: a short message shown to the member.
    pub fn toast(text: impl Into<String>) -> Self {
        Self(json!({ "toast": text.into() }))
    }

    /// An `insert` answer for `action.run` on a composer button: text for the cursor.
    pub fn insert(text: impl Into<String>) -> Self {
        Self(json!({ "insert": text.into() }))
    }

    /// An `action.run` failure the member reads (the button shows it instead of a result).
    pub fn error(message: impl Into<String>) -> Self {
        Self(json!({ "error": message.into() }))
    }

    /// A card for `onebox.resolve`; `image` must be an `https://` address.
    pub fn card(title: &str, description: &str, image: &str, site: &str) -> Self {
        Self(json!({ "title": title, "description": description, "image": image, "site": site }))
    }

    /// Any JSON the hook defines.
    pub fn raw(value: Value) -> Self {
        Self(value)
    }
}

impl From<Value> for Reply {
    fn from(v: Value) -> Self {
        Self(v)
    }
}

/// The text for `key` in the viewer's language from `strings` (what the host passes to
/// `panel.render`, `slot.render` and `action.run`, built from `locales/<code>.json`), or the key.
pub fn t<'a>(strings: &'a Value, key: &'a str) -> &'a str {
    strings.get(key).and_then(Value::as_str).unwrap_or(key)
}

/// Escapes text for HTML.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

#[doc(hidden)]
#[cfg(target_arch = "wasm32")]
pub mod glue {
    use super::{Reply, Request, json};

    pub fn alloc(len: i32) -> i32 {
        let buf = vec![0u8; usize::try_from(len).unwrap_or(0)].into_boxed_slice();
        Box::leak(buf).as_mut_ptr() as usize as i32
    }

    /// Reads the request at `ptr`, runs `f`, and returns the answer as `(ptr << 32) | len`.
    pub fn handle(ptr: i32, len: i32, f: fn(Request) -> Reply) -> i64 {
        // SAFETY: the host wrote `len` bytes at `ptr`, which `alloc` handed out, just before this call.
        let input = unsafe { core::slice::from_raw_parts(ptr as usize as *const u8, usize::try_from(len).unwrap_or(0)) };
        let out = match serde_json::from_slice::<Request>(input) {
            Ok(req) => f(req).0,
            Err(e) => json!({ "error": e.to_string() }),
        };
        let bytes = serde_json::to_vec(&out).unwrap_or_else(|_| b"{}".to_vec());
        let len = bytes.len();
        let ptr = Box::leak(bytes.into_boxed_slice()).as_ptr() as usize;
        ((ptr as i64) << 32) | len as i64
    }
}

/// Exports `alloc` and `handle`, the two functions the host looks for, around `$handler`
/// (`fn(Request) -> Reply`). Use it once, at the top level of the plugin crate.
#[macro_export]
macro_rules! export {
    ($handler:path) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn alloc(len: i32) -> i32 {
            $crate::glue::alloc(len)
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn handle(ptr: i32, len: i32) -> i64 {
            $crate::glue::handle(ptr, len, $handler)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_have_the_shapes_the_host_reads() {
        assert_eq!(Reply::reject("no").0, json!({ "reject": "no" }));
        assert_eq!(Reply::data(json!({ "raw": "x" })).0, json!({ "data": { "raw": "x" } }));
        assert_eq!(Reply::json(201, json!([1])).0, json!({ "status": 201, "json": [1] }));
        assert_eq!(Reply::none().0, json!({}));
    }

    #[test]
    fn requests_parse_with_missing_parts() {
        let r: Request = serde_json::from_str(r#"{"hook":"job.run"}"#).unwrap();
        assert_eq!(r.hook, "job.run");
        assert_eq!(r.setting_str("x", "d"), "d");
        let r: Request = serde_json::from_str(r#"{"hook":"h","data":{"raw":"a"},"settings":{"n":3}}"#).unwrap();
        assert_eq!(r.payload::<hooks::PostBeforeCreate>().unwrap().raw, "a");
        assert_eq!(r.setting_i64("n", 0), 3);
    }

    #[test]
    fn text_is_escaped() {
        assert_eq!(esc("<a href=\"x\">&'"), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;");
        assert_eq!(t(&json!({ "a": "A" }), "a"), "A");
        assert_eq!(t(&json!({}), "a"), "a");
    }
}
