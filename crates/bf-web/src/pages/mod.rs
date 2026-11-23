//! Page view models. Every page carries a `Layout` for the shared `<head>` and chrome.
//!
//! One file per area; everything is re-exported here so callers use `pages::Name`.

mod account;
mod community;
mod forum;
mod layout;
mod messages;
mod moderation;
mod pager;
mod profile;

pub use account::*;
pub use community::*;
pub use forum::*;
pub use layout::*;
pub use messages::*;
pub use moderation::*;
pub use pager::page_window;
pub use profile::*;

use askama::Template;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

#[derive(Template)]
#[template(path = "styleguide.html")]
pub struct StyleguidePage {
    pub layout: Layout,
    pub icons: Vec<&'static str>,
}

/// The site's header on its own, for the admin panel, which is not rendered here.
#[derive(Template)]
#[template(path = "_header.html")]
pub struct HeaderFragment {
    pub layout: Layout,
}

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorPage {
    pub layout: Layout,
    pub status: u16,
    pub message: String,
}

impl ErrorPage {
    /// The page's heading in words: what kind of error, or for a 2xx notice the page title.
    pub fn heading(&self) -> String {
        let t = &self.layout.t;
        match self.status {
            200..=299 => {
                let site = format!(" - {}", self.layout.site_name);
                self.layout.title.strip_suffix(&site).unwrap_or(&self.layout.title).to_owned()
            }
            404 => t.msg("Not found"),
            403 => t.msg("Not allowed"),
            400 | 422 => t.msg("Please check that"),
            429 => t.msg("Slow down"),
            503 => t.msg("Under maintenance"),
            500..=599 => t.msg("Something went wrong"),
            _ => t.msg("Error"),
        }
    }
}

/// Renders a template, turning a (should-never-happen) render failure into a plain 500.
pub fn render(t: &impl Template) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, [(header::CONTENT_TYPE, "text/plain")], "Internal server error")
                .into_response()
        }
    }
}

/// Where a topic page is split for streaming.
pub const SUGGESTED_MARKER: &str = "<!--bf-suggested-->";

pub fn render_string(t: &impl Template) -> Result<String, askama::Error> {
    t.render()
}
