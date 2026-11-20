//! Building blocks for server-rendered pages: the page shell (`begin` … `finish`), forms and
//! fields, tables and times. Pages build their HTML from these
//! instead of each writing its own; everything that takes text escapes it.

use crate::http::i18n::trh;
use crate::http::render::{ago, layout, page};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::http::HeaderValue;
use axum::response::Response;
use bf_core::html::esc;
use bf_web::pages::{AppPage, Layout, render};
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// The parts of a page around the page's own HTML.
#[derive(Default)]
pub(crate) struct Shell {
    pub title: String,
    pub crumbs: Vec<(String, String)>,
    /// Escaped or cooked HTML under the heading.
    pub intro: String,
    /// Buttons beside the heading.
    pub actions: String,
    pub canonical: Option<String>,
    /// Plain text for the meta description.
    pub description: Option<String>,
    pub feed: Option<String>,
    pub noindex: bool,
    pub json_ld: Vec<String>,
    /// Open Graph type and image for this page, when it is not the site's own.
    pub og_type: String,
    pub og_image: Option<String>,
    /// Widget areas (the page builder): the area prefix, its widgets, and whether the layout
    /// editor is open.
    pub area: String,
    pub widgets: bf_web::pages::PageWidgets,
    pub editing_layout: bool,
    /// A custom page's layout (`single`, `sidebar`, `columns2`, `columns3`); empty for other pages.
    pub page_template: String,
    /// The page has no sidebar area, whatever is in `side`.
    pub no_side: bool,
}

impl Shell {
    /// Gives the page widget areas named `{area}_top`, `_side` and `_bottom`.
    pub(crate) async fn with_area(mut self, state: &AppState, area: &str, editing: bool) -> Self {
        self.widgets = crate::admin::themes::page_widgets(state, area).await;
        self.area = area.to_owned();
        self.editing_layout = editing;
        self
    }
}

/// `?edit_layout=1`: an admin opened the layout editor (the page builder). Any handler can take
/// it; it never fails, and it is only acted on for admins.
pub(crate) struct EditLayout(pub bool);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for EditLayout {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(parts.uri.query().is_some_and(|q| q.split('&').any(|p| p == "edit_layout=1"))))
    }
}

/// A page being built: the layout comes first, so forms use the same CSRF token it sends.
pub(crate) struct Ctx {
    layout: Layout,
    set: Option<HeaderValue>,
    pub token: String,
}

impl Ctx {
    /// An icon from the sprite, for HTML built in handlers.
    pub(crate) fn icon(&self, name: &str) -> String {
        self.layout.icon(name)
    }
}

pub(crate) fn begin(state: &AppState, v: &Viewer) -> Ctx {
    let (layout, set) = layout(state, v, None);
    let token = layout.csrf.clone();
    Ctx { layout, set, token }
}

pub(crate) fn finish(state: &AppState, v: &Viewer, ctx: Ctx, shell: Shell, html: String) -> Response {
    let mut l = ctx.layout;
    l.title = format!("{} - {}", shell.title, l.site_name);
    if let Some(c) = shell.canonical {
        l.canonical = Some(format!("{}{c}", state.config.base_url));
    }
    if let Some(d) = shell.description.filter(|d| !d.trim().is_empty()) {
        l.description = d.chars().take(300).collect();
    }
    l.feed = shell.feed;
    l.noindex = shell.noindex;
    l.json_ld = shell.json_ld;
    l.og_type = shell.og_type;
    l.og_image = shell.og_image;
    let cacheable = v.user.is_none() && !shell.noindex;
    let can_edit_layout = v.actor.is_admin && !shell.area.is_empty();
    let body = AppPage {
        layout: l,
        crumbs: shell.crumbs,
        heading: shell.title,
        intro: shell.intro,
        actions: shell.actions,
        html,
        area: shell.area,
        widgets: shell.widgets,
        can_edit_layout,
        editing_layout: can_edit_layout && shell.editing_layout,
        page_template: shell.page_template,
        no_side: shell.no_side,
    };
    page(render(&body), ctx.set, cacheable)
}

/// A page of plain content in the shared shell: a heading and one box. `index` lets search engines
/// list it. For a page with forms, build it with `begin` and `finish`, which give the CSRF token.
pub(crate) fn simple_page(state: &AppState, v: &Viewer, title: &str, html: &str, index: bool) -> Response {
    let ctx = begin(state, v);
    finish(state, v, ctx, Shell { title: title.into(), noindex: !index, ..Shell::default() }, panel(html))
}

/// Content in one box, the way simple pages show it.
pub(crate) fn panel(html: &str) -> String {
    format!("<section class=\"bf-box block bf-panel\">{html}</section>")
}

// Components: the small pieces most pages are built from. Text arguments are written in English
// and shown in the viewer's language (`tr`; scripts/i18n.py finds them at the call sites), then
// escaped; arguments named `html` are markup the caller built (escaped or cooked).

/// A data table: column headings (escaped), and rows of cells (HTML), or one row saying `empty`
/// when there are none.
pub(crate) fn table(headings: &[&str], rows: &[Vec<String>], empty: &str) -> String {
    let mut out = String::from("<table class=\"bf-table\"><thead><tr>");
    for h in headings {
        out.push_str(&format!("<th>{}</th>", trh(h)));
    }
    out.push_str("</tr></thead><tbody>");
    for row in rows {
        out.push_str("<tr>");
        for cell in row {
            out.push_str(&format!("<td>{cell}</td>"));
        }
        out.push_str("</tr>");
    }
    if rows.is_empty() {
        out.push_str(&empty_row(headings.len(), empty));
    }
    out.push_str("</tbody></table>");
    out
}

/// A table in its own box, under a heading (none when `title` is empty).
pub(crate) fn table_box(title: &str, headings: &[&str], rows: &[Vec<String>], empty: &str) -> String {
    let head = if title.is_empty() { String::new() } else { format!("<h2>{}</h2>", trh(title)) };
    format!("<section class=\"bf-box block\">{head}{}</section>", table(headings, rows, empty))
}

/// The row a table shows when it has no rows.
pub(crate) fn empty_row(columns: usize, text: &str) -> String {
    format!("<tr><td colspan=\"{columns}\" class=\"muted\">{}</td></tr>", trh(text))
}

pub(crate) fn hidden_csrf(token: &str) -> String {
    format!("<input type=\"hidden\" name=\"csrf\" value=\"{}\">", esc(token))
}

pub(crate) fn when(t: DateTime<Utc>) -> String {
    ago(t).to_string()
}

/// A labelled Markdown field. The value is escaped.
pub(crate) fn md_field(name: &str, label: &str, value: &str, rows: u8, required: bool) -> String {
    format!(
        "<label class=\"bf-field\"><textarea name=\"{name}\" rows=\"{rows}\" placeholder=\" \"{}>{}</textarea><span class=\"bf-field-label\">{}</span></label>",
        if required { " required" } else { "" },
        esc(value),
        trh(label)
    )
}

pub(crate) fn text_field(name: &str, label: &str, value: &str, max: usize, required: bool) -> String {
    format!(
        "<label class=\"bf-field\"><input name=\"{name}\" value=\"{}\" maxlength=\"{max}\" placeholder=\" \"{}><span class=\"bf-field-label\">{}</span></label>",
        esc(value),
        if required { " required" } else { "" },
        trh(label)
    )
}

/// `?edit_layout=1`.
#[derive(Deserialize, Default)]
pub(crate) struct PageQuery {
    #[serde(default)]
    pub edit_layout: Option<String>,
}

impl PageQuery {
    pub(crate) fn editing(&self) -> bool {
        self.edit_layout.as_deref() == Some("1")
    }
}

/// A member's name as a link to their profile, for HTML built in handlers. A removed account
/// (`None` or the stored stand-in "deleted") is the translated words "deleted member", unlinked.
pub(crate) fn user_link(name: Option<&str>) -> String {
    match name.filter(|n| !n.is_empty() && *n != "deleted") {
        Some(n) => {
            format!("<a class=\"bf-user-link\" href=\"{}\">{}</a>", esc(&bf_core::slug::profile_path(0, n)), esc(n))
        }
        None => crate::http::i18n::trh("deleted member"),
    }
}
