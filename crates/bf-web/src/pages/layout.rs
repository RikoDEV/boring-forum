//! The page shell: legal-document flags, branding and the `Layout` every page carries.

use crate::assets::{BRANDS, CSS, EDITOR, ICONS, ISLANDS, MEMBERS};
use bf_core::settings::Settings;
use std::sync::atomic::{AtomicU8, Ordering};

use super::PageArea;

/// Bits of `LegalDocs`, kept for the whole process: loaded at startup, updated on publish.
static LEGAL_PUBLISHED: AtomicU8 = AtomicU8::new(0);

/// Which legal documents exist. Unpublished ones answer 404, so nothing links to them.
#[derive(Clone, Copy, Debug, Default)]
pub struct LegalDocs {
    pub terms: bool,
    pub privacy: bool,
    pub guidelines: bool,
}

impl LegalDocs {
    fn bit(kind: &str) -> u8 {
        match kind {
            "terms" => 1,
            "privacy" => 2,
            "guidelines" => 4,
            _ => 0,
        }
    }

    pub fn current() -> Self {
        let b = LEGAL_PUBLISHED.load(Ordering::Relaxed);
        Self { terms: b & 1 != 0, privacy: b & 2 != 0, guidelines: b & 4 != 0 }
    }

    /// Replaces the record with exactly these kinds.
    pub fn set<'a>(kinds: impl IntoIterator<Item = &'a str>) {
        LEGAL_PUBLISHED.store(kinds.into_iter().fold(0, |b, k| b | Self::bit(k)), Ordering::Relaxed);
    }

    /// Marks one more kind as published.
    pub fn mark(kind: &str) {
        LEGAL_PUBLISHED.fetch_or(Self::bit(kind), Ordering::Relaxed);
    }
}

/// The site's own pictures. Settings validation keeps each one an upload path or empty
/// (`bf_core::settings::upload_path_ok`), so they go into attributes as they are.
#[derive(Clone, Debug, Default)]
pub struct Brand {
    pub logo: String,
    pub logo_dark: String,
    pub logo_small: String,
    pub favicon: String,
    pub large_icon: String,
    pub og_image: String,
}

impl Brand {
    pub fn from_settings(s: &Settings) -> Self {
        let get = |k: &str| s.str(k).to_owned();
        Self {
            logo: get("logo_url"),
            logo_dark: get("logo_dark_url"),
            logo_small: get("logo_small_url"),
            favicon: get("favicon_url"),
            large_icon: get("large_icon_url"),
            og_image: get("og_image_url"),
        }
    }
}

/// Who has been on a page lately: members by name, guests as a count, crawlers by name.
#[derive(Debug, Clone, Default)]
pub struct Browsing {
    /// (name, profile url), newest first.
    pub members: Vec<(String, String)>,
    pub guests: usize,
    /// How many different crawlers; `bots` lists the latest few by name.
    pub bot_count: usize,
    pub bots: Vec<String>,
}

pub struct Layout {
    pub ad_source: Option<String>,
    /// Content translation is on: posts offer a "Translate" link to those who may edit them.
    pub translations: bool,
    /// AI features that add buttons or blocks to pages (each needs `ai_enabled` too).
    pub ai_summaries: bool,
    /// A topic needs more posts than this to offer a summary.
    pub ai_summary_min: i32,
    pub ai_translate: bool,
    pub ai_related: bool,
    pub ai_helper: bool,
    /// The longest post, in characters, and whether `$math$` is rendered: the editor shows a
    /// counter and offers the math tools.
    pub post_max: i64,
    pub math: bool,
    /// The GIF picker's settings when it is on and has a key. The browser asks the provider itself.
    pub gifs: Option<GifConfig>,
    /// Optional modules that are on: (url, label), listed in the footer so each is always reachable.
    pub modules: Vec<(String, String)>,
    /// Account menu extras: referral links, the fediverse page, selling.
    pub referrals: bool,
    pub fediverse: bool,
    /// The viewer's groups, for choosing ads; guests are group 1.
    pub ad_groups: Vec<i64>,
    /// Analytics: the provider's script tag, or the settings for the consent-gated one.
    pub analytics: crate::security::Analytics,
    pub site_name: String,
    pub title: String,
    /// What this page says about itself for search results and link previews: the topic's start, a forum's
    /// description. Pages set it; the footer and the about page use `site_description` instead.
    pub description: String,
    /// The site's own description (the `site_description` setting), the same on every page.
    pub site_description: String,
    pub css_url: String,
    pub islands_url: String,
    /// Editor bundle, loaded by the composer island on first focus.
    pub editor_url: String,
    /// Member-only islands bundle.
    pub members_url: String,
    /// Signed-in username; `None` for guests.
    pub user: Option<String>,
    /// The viewer's public profile.
    pub user_url: String,
    pub unread_notifications: i64,
    pub unread_messages: i64,
    pub unread_chat: i64,
    /// Shows the review queue link (moderators and admins).
    pub moderator: bool,
    /// Site-wide staff (moderators group and admins), not the moderator of one forum or club.
    pub staff: bool,
    pub admin: bool,
    pub review_count: i64,
    /// Double-submit CSRF token echoed into every form.
    pub csrf: String,
    pub canonical: Option<String>,
    pub noindex: bool,
    /// The neighbouring pages of a paginated list, as absolute addresses, for `<link rel=prev/next>`.
    pub prev_link: Option<String>,
    pub next_link: Option<String>,
    /// Pre-escaped JSON-LD (bf_seo::json_ld_script).
    pub json_ld: Vec<String>,
    /// Open Graph type for pages that are articles; empty means `website`.
    pub og_type: String,
    /// Open Graph image (an absolute address) for one page; the site-wide picture when `None`.
    pub og_image: Option<String>,
    /// The look of drawn share pictures when the site has them switched on.
    pub og_style: Option<bf_core::og::Style>,
    /// Translator for this request's language.
    pub t: bf_core::i18n::T,
    /// Text size in percent and density, from the member's interface settings.
    pub text_size: i16,
    pub density: String,
    /// Header links as (url, label), filtered for this viewer.
    pub nav: Vec<(String, String)>,
    pub sidebar: Vec<(String, String)>,
    /// Links plugins declared with an icon or a menu, for this viewer: (url, label, icon, menu).
    pub plugin_menu: Vec<(String, String, String, String)>,
    /// The chosen theme's stylesheet and rendered slots, if any.
    pub theme: Option<std::sync::Arc<crate::theme::Compiled>>,
    /// `None` in safe mode.
    pub plugins: Option<std::sync::Arc<crate::theme::PluginOutput>>,
    /// Widget areas around the content, for pages that opt in (see `PageArea`).
    pub area: Option<PageArea>,
    /// Custom CSS and HTML for every page (ACP → Settings → Custom code); `None` in safe mode.
    pub custom: Option<std::sync::Arc<crate::custom_code::CustomCode>>,
    /// The member chose infinite scroll.
    pub infinite: bool,
    pub chat_enabled: bool,
    /// Who is on this page lately. None where the box is not shown.
    pub browsing: Option<Browsing>,
    /// This page's RSS feed, when guests can read it.
    pub feed: Option<String>,
    /// The EU DSA notice form is open.
    pub dsa_notices: bool,
    /// Which legal documents have been published, so unpublished ones get no link.
    pub legal: LegalDocs,
    /// The site's pictures (ACP → Settings → Branding); each is an upload path or empty.
    pub brand: Brand,
    /// "classic" (author column) or "discourse" (avatar beside the text).
    pub post_style: &'static str,
    /// How the forum index lists forums: list, boxes, compact or latest.
    pub forum_layout: &'static str,
    pub date_format: String,
    pub impersonating: bool,
    /// Updated terms await this member's acceptance: every page opens the terms dialog.
    pub legal_pending: bool,
    /// The getting-started banner is shown (new member, steps left, not dismissed).
    pub welcome: bool,
    pub scheme: String,
    /// The colour schemes' stylesheet, linked after the built-in CSS.
    pub scheme_css: Option<String>,
    pub skin_tone: i16,
    pub cookie_text: String,
    /// Tokens, base and shell CSS, inlined so the first paint needs no request.
    pub critical_css: &'static str,
    pub loader: String,
}

impl Layout {
    /// A counted phrase as (number, rest), so the number can be styled on its own.
    pub fn count_parts(&self, key: &str, n: impl bf_core::i18n::Count) -> (String, String) {
        let num = n.count().to_string();
        let text = self.t.n(key, n);
        // Around the number, wherever the language puts it; a form written in words (Arabic
        // "رد واحد", one reply) has no number to set apart.
        match text.split_once(num.as_str()) {
            Some((before, after)) => (num, format!("{}{}", before.trim(), after.trim())),
            None => (String::new(), text),
        }
    }

    /// A category or forum's icon: its picture, emoji or sprite icon, else `fallback`. Values are
    /// checked when saved, and again here since this goes into the page unescaped.
    pub fn node_icon(&self, icon: &Option<String>, image: &Option<String>, fallback: &str) -> String {
        if let Some(src) = image.as_deref().filter(|s| s.starts_with("/uploads/") && !s.contains(['"', '<', '>', '&']))
        {
            return format!(r#"<img src="{src}" alt="" width="48" height="48" loading="lazy">"#);
        }
        match icon.as_deref().filter(|i| !i.is_empty()) {
            Some(i) if i.chars().all(|c| !c.is_ascii() && !c.is_whitespace()) => format!("<span>{i}</span>"),
            Some(i) if i.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') => self.icon(i),
            _ => self.icon(fallback),
        }
    }

    /// An inline reference to one icon of the sprite. Names come from templates, not requests.
    /// A badge's medal: a coloured gradient, a shape for its tier, its icon, and its number.
    pub fn medal(
        &self,
        tier: &str,
        icon: &str,
        color: impl std::fmt::Display,
        level: impl std::fmt::Display,
    ) -> String {
        let shape = match tier {
            "gold" => "gold",
            "silver" => "silver",
            _ => "bronze",
        };
        // Templates hand these over behind any number of references; only digits get through.
        let level: i32 = level.to_string().parse().unwrap_or(0);
        let chip = match level {
            0 => String::new(),
            n if n >= 1000 && n % 1000 == 0 => format!(r#"<span class="bf-medal-level">{}k</span>"#, n / 1000),
            n => format!(r#"<span class="bf-medal-level">{n}</span>"#),
        };
        // Colour numbers come from the database, so only a known one becomes a class.
        let color = color.to_string().parse::<i32>().unwrap_or(0).rem_euclid(10);
        format!(
            r#"<span class="bf-medal bf-medal-{shape}" title="{shape}"><span class="bf-medal-shape bf-medal-c{color}">{}</span>{chip}</span>"#,
            self.icon(icon)
        )
    }

    /// A member's name as a link to their profile (which also opens their card on hover). A
    /// deleted account, or no name, stays plain text.
    pub fn user_link(&self, name: &str) -> String {
        if name == "deleted" {
            // The stored stand-in for a removed account, shown in the reader's language.
            return bf_core::html::esc(&self.t.msg("deleted member"));
        }
        let shown = bf_core::html::esc(name);
        if name.is_empty() {
            return shown;
        }
        let href = bf_core::html::esc(&bf_core::slug::profile_path(0, name));
        format!(r#"<a class="bf-user-link" href="{href}">{shown}</a>"#)
    }

    /// The neighbouring pages of a paginated list for the head's `rel=prev` and `rel=next` links;
    /// the addresses are site-relative and are made absolute here.
    pub fn set_paging(&mut self, base_url: &str, prev: Option<&str>, next: Option<&str>) {
        let abs = |u: &str| format!("{}{u}", base_url.trim_end_matches('/'));
        self.prev_link = prev.map(abs);
        self.next_link = next.map(abs);
    }

    /// "by {name}" in the viewer's language, with the name linked.
    pub fn by_line(&self, name: &str) -> String {
        const MARK: &str = "\u{1}";
        bf_core::html::esc(&self.t.f("list.by", &[("name", MARK)])).replace(MARK, &self.user_link(name))
    }

    /// The brand logo sprite, for the icon picker on the site (islands icon-picker.js).
    pub fn brand_sprite(&self) -> String {
        BRANDS.url()
    }

    pub fn icon(&self, name: &str) -> String {
        static URL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| ICONS.url());
        static BRAND_URL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| BRANDS.url());
        // Brand logos are filled shapes in their own sprite; the class lets CSS drop the stroke.
        if name.starts_with("brand-") {
            return format!(
                r#"<svg class="bf-icon bf-brand" aria-hidden="true"><use href="{}#{name}"></use></svg>"#,
                *BRAND_URL
            );
        }
        format!(r#"<svg class="bf-icon" aria-hidden="true"><use href="{}#{name}"></use></svg>"#, *URL)
    }

    /// A theme slot's HTML, already sanitized; empty when no theme fills it.
    /// A theme's replacement for a core partial, or `None` to use the compiled one. A broken
    /// override also gives `None`: the page falls back rather than losing that part.
    pub fn partial(&self, name: &str, pairs: &[(&str, &str)]) -> Option<String> {
        let src = self.theme.as_ref()?.partials.get(name)?;
        let ctx: serde_json::Map<String, serde_json::Value> =
            pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).into())).collect();
        crate::theme::render_slot(src, &serde_json::Value::Object(ctx))
            .map_err(|e| tracing::warn!(partial = name, error = %e, "theme partial failed"))
            .ok()
    }

    /// Every language the site speaks: (code, native name, text direction), for the picker.
    pub fn languages(&self) -> Vec<(&'static str, &'static str, &'static str)> {
        bf_core::i18n::locales().into_iter().map(|(c, n)| (c, n, bf_core::i18n::T::new(c).dir())).collect()
    }

    /// The viewer's language by its own name ("Polski").
    pub fn language_name(&self) -> &'static str {
        bf_core::i18n::locales().into_iter().find(|(c, _)| *c == self.t.locale).map_or("English", |(_, n)| n)
    }

    /// The messages the islands show (catalog keys under `js.`), in the viewer's language, for
    /// `t()` in dom.js. `<` cannot occur inside the script element this way.
    pub fn client_messages(&self) -> String {
        // The islands' own texts, and the counted phrases ("{n} posts") they share with the pages.
        let mut m = self.t.messages("js.");
        m.extend(self.t.messages("count."));
        serde_json::to_string(&m).unwrap_or_else(|_| "{}".into()).replace('<', "\\u003c")
    }

    /// JSON the page loader reads to decide which plugin modules this page needs.
    pub fn plugin_modules(&self) -> String {
        let Some(p) = &self.plugins else { return "[]".into() };
        let rows: Vec<_> =
            p.scripts.iter().map(|(src, islands)| serde_json::json!({ "src": src, "islands": islands })).collect();
        // `<` cannot occur inside a script element's JSON this way.
        serde_json::Value::Array(rows).to_string().replace('<', "\\u003c")
    }

    /// Adds a member's own links to the sidebar, and links from plugins to the header.
    pub fn add_links(&mut self, own_sidebar: Option<&str>, groups: &[i64], is_admin: bool) {
        if let Some(own) = own_sidebar.filter(|o| !o.trim().is_empty()) {
            let items = bf_core::nav::visible(bf_core::nav::parse(own), groups, is_admin);
            self.sidebar.extend(items.into_iter().filter(|i| i.key.is_none()).map(|i| (i.url, i.label)));
        }
        if let Some(p) = &self.plugins {
            let locale = self.t.locale;
            // Links with an icon or menu are drawn by `plugin_nav_header` and `plugin_nav_html`.
            let rich_urls: Vec<&str> = p.extras.nav_rich.iter().map(|r| r.item.url.as_str()).collect();
            self.nav.extend(
                p.nav
                    .iter()
                    .filter(|(u, _)| !rich_urls.contains(&u.as_str()))
                    .map(|(u, l)| (u.clone(), crate::theme::plugin_label(locale, l))),
            );
            // Links with a visibility rule (`group`, `groups`) show only to those who may use them.
            let items = bf_core::nav::visible(p.extras.nav_items.clone(), groups, is_admin);
            self.nav.extend(items.into_iter().map(|i| (i.url, crate::theme::plugin_label(locale, &i.label))));
            // Links with an icon or a menu: ordered, and only for those who may use them.
            let mut rich: Vec<_> = p
                .extras
                .nav_rich
                .iter()
                .filter(|r| !bf_core::nav::visible(vec![r.item.clone()], groups, is_admin).is_empty())
                .collect();
            rich.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.item.label.cmp(&b.item.label)));
            self.plugin_menu = rich
                .into_iter()
                .map(|r| {
                    (
                        r.item.url.clone(),
                        crate::theme::plugin_label(locale, &r.item.label),
                        r.icon.clone(),
                        crate::theme::plugin_label(locale, &r.menu),
                    )
                })
                .collect();
        }
    }

    /// The same links for the header bar: each menu as a drop-down, the rest as links with icons.
    pub fn plugin_nav_header(&self) -> String {
        use bf_core::html::esc;
        let link = |(url, label, icon, _): &(String, String, String, String)| {
            let glyph = if icon.is_empty() { String::new() } else { self.icon(icon) };
            format!("<a class=\"bf-nav-rich\" href=\"{}\">{glyph}{}</a>", esc(url), esc(label))
        };
        let mut out = String::new();
        let mut menus: Vec<&str> = Vec::new();
        for item in self.plugin_menu.iter().filter(|i| !i.3.is_empty()) {
            if !menus.contains(&item.3.as_str()) {
                menus.push(&item.3);
            }
        }
        for menu in menus {
            let links: String = self.plugin_menu.iter().filter(|i| i.3 == menu).map(&link).collect();
            out.push_str(&format!(
                "<details class=\"bf-menu bf-nav-menu\"><summary>{}{}</summary><div class=\"bf-menu-body\">{links}</div></details>",
                esc(menu),
                self.icon("chevron-down")
            ));
        }
        self.plugin_menu.iter().filter(|i| i.3.is_empty()).for_each(|i| out.push_str(&link(i)));
        out
    }

    /// The links plugins grouped under menus or gave icons, as HTML: a heading for each menu, then
    /// its links, then those without a menu.
    pub fn plugin_nav_html(&self) -> String {
        use bf_core::html::esc;
        let link = |(url, label, icon, _): &(String, String, String, String)| {
            let glyph = if icon.is_empty() { String::new() } else { self.icon(icon) };
            format!("<a class=\"bf-nav-rich\" href=\"{}\">{glyph}{}</a>", esc(url), esc(label))
        };
        let mut out = String::new();
        let mut menus: Vec<&str> = Vec::new();
        for item in self.plugin_menu.iter().filter(|i| !i.3.is_empty()) {
            if !menus.contains(&item.3.as_str()) {
                menus.push(&item.3);
            }
        }
        for menu in menus {
            out.push_str(&format!("<strong class=\"bf-nav-group\">{}</strong>", esc(menu)));
            self.plugin_menu.iter().filter(|i| i.3 == menu).for_each(|i| out.push_str(&link(i)));
        }
        self.plugin_menu.iter().filter(|i| i.3.is_empty()).for_each(|i| out.push_str(&link(i)));
        out
    }

    /// `<link>` tags for the style sheets plugins ship.
    pub fn plugin_css(&self) -> String {
        let Some(p) = &self.plugins else { return String::new() };
        p.extras.css.iter().map(|u| format!("<link rel=\"stylesheet\" href=\"{u}\">")).collect()
    }

    /// JSON the islands read for the buttons plugins declare (`[[post_actions]]`,
    /// `[[composer_buttons]]`), with their labels in the viewer's language.
    pub fn plugin_actions(&self) -> String {
        let Some(p) = &self.plugins else { return "[]".into() };
        let locale = self.t.locale;
        let rows: Vec<_> = p
            .extras
            .actions
            .iter()
            .map(|a| {
                let mut a = a.clone();
                if let Some(l) = a.get("label").and_then(|l| l.as_str()).map(|l| crate::theme::plugin_label(locale, l)) {
                    a["label"] = l.into();
                }
                a
            })
            .collect();
        serde_json::Value::Array(rows).to_string().replace('<', "\\u003c")
    }

    /// The site's advert, without scripts.
    pub fn ad(&self) -> String {
        let legacy = self.ad_source.as_deref().map(crate::theme::clean_html).unwrap_or_default();
        format!("{legacy}{}", self.ad_slot("header"))
    }

    /// The ad for one place on the page, if any.
    pub fn ad_slot(&self, placement: &str) -> String {
        crate::ads::pick(placement, &self.ad_groups)
            .map(|html| {
                // The label is added here, in the reader's language, not stored with the ad.
                let label = bf_core::html::esc(&self.t.msg("Advertisement"));
                format!("<div class=\"bf-ad bf-ad-{placement}\"><span class=\"bf-ad-label small muted\">{label}</span>{html}</div>")
            })
            .unwrap_or_default()
    }

    /// An ad after every n-th post (`index` counts from 1), never after the last one shown.
    pub fn ad_between(&self, index: impl std::borrow::Borrow<usize>, count: impl std::borrow::Borrow<usize>) -> String {
        let (index, count) = (*index.borrow(), *count.borrow());
        if index >= count {
            return String::new();
        }
        match crate::ads::between_every(&self.ad_groups) {
            Some(every) if index % every == 0 => self.ad_slot("between_posts"),
            _ => String::new(),
        }
    }

    /// The announcements running now, in the reader's language where an administrator translated them.
    pub fn announcements(&self) -> Vec<crate::theme::Announcement> {
        crate::theme::current_announcements()
            .into_iter()
            .map(|mut a| {
                a.title = self.t.msg(&a.title);
                if !a.body.trim().is_empty() {
                    let body = self.t.msg(&a.body);
                    if body != a.body {
                        a.html = crate::theme::cook_cached(&body);
                    }
                }
                a
            })
            .collect()
    }

    /// Links to plugin pages for one area: (url, label).
    pub fn plugin_links(&self, area: &str) -> Vec<(String, String)> {
        let Some(p) = &self.plugins else { return Vec::new() };
        let locale = self.t.locale;
        p.panels
            .iter()
            .filter(|x| x.0 == area)
            .map(|x| (x.1.clone(), crate::theme::plugin_label(locale, &x.2)))
            .collect()
    }

    /// Reactions members can choose: (id, name, icon).
    pub fn reactions(&self) -> Vec<(i64, String, String)> {
        crate::theme::reactions().iter().map(|(id, name, icon)| (*id, self.t.msg(name), icon.clone())).collect()
    }

    /// The report form's reasons: (key, label, needs a note).
    pub fn report_reasons(&self) -> Vec<(String, String, bool)> {
        let list = crate::theme::report_reasons();
        if list.is_empty() {
            // Before the first load from the database.
            return vec![("other".into(), "Something else".into(), true)];
        }
        list.iter().map(|(key, label, note)| (key.clone(), self.t.msg(label), *note)).collect()
    }

    pub fn slot(&self, name: &str) -> String {
        let theme = self.theme.as_ref().and_then(|t| t.slots.get(name)).map_or("", String::as_str);
        let plugin = self.plugins.as_ref().and_then(|p| p.slots.get(name)).map_or("", String::as_str);
        // A slot a plugin draws per viewer is filled in by the page script (`slot.render`).
        let dynamic = self.plugins.as_ref().is_some_and(|p| p.extras.dynamic_slots.contains(name));
        let holder = if dynamic { format!("<div class=\"bf-plugin-slot\" data-plugin-slot=\"{name}\"></div>") } else { String::new() };
        format!("{theme}{plugin}{holder}")
    }

    /// Fills `nav` from the `nav_links` setting for a viewer in `groups`.
    pub fn set_nav(&mut self, settings: &Settings, groups: &[i64], is_admin: bool) {
        // A module's link waits until the module is on.
        let on = |i: &bf_core::nav::NavItem| {
            i.key.as_deref().and_then(bf_core::nav::module_setting).is_none_or(|s| settings.bool(s))
        };
        self.modules = bf_core::nav::MODULES
            .iter()
            .filter(|m| settings.bool(m.2))
            .map(|m| (m.1.to_owned(), self.t.t(&format!("nav.{}", m.0))))
            .collect();
        let items: Vec<_> = bf_core::nav::visible(bf_core::nav::parse(settings.str("nav_links")), groups, is_admin)
            .into_iter()
            .filter(on)
            .collect();
        let side = settings.str("sidebar_links");
        self.sidebar = if side.trim().is_empty() {
            Vec::new()
        } else {
            bf_core::nav::visible(bf_core::nav::parse(side), groups, is_admin)
                .into_iter()
                .filter(on)
                .map(|i| {
                    let label = i.key.as_ref().map_or_else(|| self.t.msg(&i.label), |k| self.t.t(&format!("nav.{k}")));
                    (i.url, label)
                })
                .collect()
        };
        self.nav = items
            .into_iter()
            .map(|i| {
                let label = i.key.as_ref().map_or_else(|| self.t.msg(&i.label), |k| self.t.t(&format!("nav.{k}")));
                (i.url, label)
            })
            .collect();
    }

    /// The address of the drawn share picture for this page, when the site draws them for pages
    /// of this kind: a public page (it has a canonical address and is indexed), with no picture
    /// of its own. Built at render time from the title and description the page ends up with, so
    /// every page that shows a title gets one without doing anything.
    pub fn generated_image(&self) -> Option<String> {
        let style = self.og_style.as_ref()?;
        let canonical = self.canonical.as_deref()?;
        let kind = bf_core::og::kind_of(canonical);
        if self.noindex || !style.covers(kind) {
            return None;
        }
        let (scheme, rest) = canonical.split_once("://")?;
        let origin = format!("{scheme}://{}", rest.split('/').next()?);
        let title = self.title.strip_suffix(&format!(" - {}", self.site_name)).unwrap_or(&self.title);
        Some(format!("{origin}{}", bf_core::og::picture_path(kind, title, &self.description, &style.version(), style.extension())))
    }

    pub fn new(settings: &Settings, title: Option<&str>) -> Self {
        let site = settings.str("site_name");
        let mut layout = Self {
            site_name: site.to_owned(),
            title: title.map_or_else(|| site.to_owned(), |t| format!("{t} - {site}")),
            description: settings.str("site_description").to_owned(),
            site_description: settings.str("site_description").to_owned(),
            css_url: CSS.url(),
            islands_url: ISLANDS.url(),
            editor_url: EDITOR.url(),
            members_url: MEMBERS.url(),
            user: None,
            user_url: String::new(),
            unread_notifications: 0,
            unread_messages: 0,
            unread_chat: 0,
            moderator: false,
            staff: false,
            admin: false,
            review_count: 0,
            csrf: String::new(),
            canonical: None,
            noindex: false,
            prev_link: None,
            og_type: String::new(),
            og_image: None,
            og_style: Some(bf_core::og::Style::from_settings(settings)).filter(|o| o.enabled),
            next_link: None,
            json_ld: Vec::new(),
            t: bf_core::i18n::T::default(),
            text_size: 100,
            density: "comfortable".into(),
            nav: Vec::new(),
            sidebar: Vec::new(),
            plugin_menu: Vec::new(),
            theme: crate::theme::resolve(&[], &[]),
            plugins: Some(crate::theme::plugin_output()),
            custom: Some(crate::custom_code::from_settings(settings)).filter(|c| !c.is_empty()),
            area: None,
            ad_source: Some(settings.str("ad_html")).filter(|a| !a.trim().is_empty()).map(str::to_owned),
            ad_groups: vec![1],
            translations: settings.bool("content_translation_enabled"),
            ai_summaries: settings.bool("ai_enabled") && settings.bool("ai_summaries_enabled"),
            ai_summary_min: settings.int("ai_summary_min_replies") as i32,
            ai_translate: settings.bool("ai_enabled")
                && settings.bool("ai_translate_enabled")
                && settings.bool("content_translation_enabled"),
            ai_helper: settings.bool("ai_enabled") && settings.bool("ai_helper_enabled"),
            post_max: settings.int("max_post_length"),
            math: settings.bool("math_enabled"),
            gifs: gif_config(settings),
            ai_related: settings.bool("ai_enabled")
                && settings.bool("ai_related_topics_enabled")
                && !settings.str("ai_embedding_model").trim().is_empty(),
            modules: Vec::new(),
            referrals: settings.bool("referrals_enabled"),
            fediverse: settings.bool("federation_enabled"),
            analytics: crate::security::Analytics::from_settings(settings),
            infinite: false,
            chat_enabled: settings.bool("chat_enabled"),
            browsing: None,
            feed: None,
            dsa_notices: settings.bool("dsa_notices_enabled"),
            legal: LegalDocs::current(),
            brand: Brand::from_settings(settings),
            post_style: if settings.str("post_style") == "discourse" { "discourse" } else { "classic" },
            forum_layout: match settings.str("forum_layout") {
                "boxes" => "boxes",
                "compact" => "compact",
                "latest" => "latest",
                _ => "list",
            },
            date_format: "relative".into(),
            impersonating: false,
            legal_pending: false,
            welcome: false,
            scheme: String::new(),
            scheme_css: crate::schemes::url(),
            skin_tone: 0,
            cookie_text: settings.str("cookie_consent_text").to_owned(),
            critical_css: crate::security::critical_css(),
            loader: crate::security::loader_script(),
        };
        // Group 1 is Guests: pages built without a viewer show the guest menu.
        layout.set_nav(settings, &[1], false);
        layout
    }
}

/// The GIF picker as the page tells the editor.
#[derive(Debug, Clone)]
pub struct GifConfig {
    pub key: String,
    pub provider: &'static str,
    /// `webp` or `gif`.
    pub format: &'static str,
    /// `off`, `low`, `medium` or `high`.
    pub filter: &'static str,
    pub country: String,
    /// Most results to load while scrolling; 0 for no limit.
    pub limit: i64,
}

/// The picker's settings, or None while GIFs are off or no key is set. Values outside their
/// lists fall back to the defaults, so a stray setting cannot reach the page.
fn gif_config(s: &bf_core::settings::Settings) -> Option<GifConfig> {
    let key = s.str("gif_api_key").trim().to_owned();
    if !s.bool("gifs_enabled") || key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return None;
    }
    let country: String = s.str("gif_country").trim().to_ascii_uppercase();
    Some(GifConfig {
        key,
        provider: crate::security::gif_provider(s.str("gif_provider"))?,
        format: if s.str("gif_format") == "gif" { "gif" } else { "webp" },
        filter: ["off", "low", "medium", "high"].into_iter().find(|f| *f == s.str("gif_content_filter")).unwrap_or("high"),
        country: if country.len() == 2 && country.chars().all(|c| c.is_ascii_alphabetic()) { country } else { "US".to_owned() },
        limit: if s.bool("gif_limit_scroll") { s.int("gif_max_results").clamp(24, 1000) } else { 0 },
    })
}
