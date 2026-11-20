//! Content translation: authors, staff and the `translator_groups` write other-language
//! versions of posts (a first post's version carries the topic title). Readers in that language see
//! the version, with the original in a fold. Until the first translation exists, topic pages run
//! no query for them. Forum and category names and descriptions are not here: they are site texts,
//! translated in the admin panel's Site text page like every other text.

use bf_core::html::esc;
use crate::http::i18n::trh;
use crate::http::render::DbResult;
use crate::http::ui::{Shell, begin, finish, hidden_csrf, md_field, text_field, user_link, when};
use crate::http::viewer::{CsrfForm, Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::forum::translations as db;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/translate/{kind}/{id}", get(page).post(save))
        .route("/translate/{kind}/{id}/{locale}/delete", post(delete))
}

static HAS_POSTS: AtomicBool = AtomicBool::new(false);

/// A post translation now exists (the AI made one), so topic pages must look for them.
pub(crate) fn note_post_translation() {
    HAS_POSTS.store(true, Ordering::Relaxed);
}

pub(crate) fn enabled(state: &AppState) -> bool {
    state.settings().bool("content_translation_enabled")
}

/// At startup and after every change.
pub(crate) async fn load(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    HAS_POSTS.store(db::any(pool).await.unwrap_or(false), Ordering::Relaxed);
}

/// A forum or category's name and description (as HTML) in the reader's language: the site text
/// override of each, or the stored ones.
pub(crate) fn node_text(t: &bf_core::i18n::T, n: &bf_db::forum::nodes::Node) -> (String, String) {
    let title = t.msg(&n.title);
    if n.description_md.trim().is_empty() {
        return (title, n.description_html.clone());
    }
    let md = t.msg(&n.description_md);
    if md == n.description_md {
        return (title, n.description_html.clone());
    }
    let html = bf_web::theme::cook_cached(&md);
    (title, html)
}

/// Swaps in this language's versions of the posts on a page, keeping the original in a fold,
/// and the topic title when the first post has one.
pub(crate) async fn apply_posts(
    state: &AppState,
    locale: &str,
    posts: &mut [bf_web::pages::PostView],
    title: &mut String,
) {
    if !HAS_POSTS.load(Ordering::Relaxed) || !enabled(state) {
        return;
    }
    let Some(pool) = &state.pool else { return };
    let ids: Vec<i64> = posts.iter().map(|p| p.id).collect();
    let rows = match db::for_posts(pool, &ids, locale).await {
        Ok(r) if !r.is_empty() => r,
        _ => return,
    };
    for p in posts.iter_mut() {
        let Some(t) = rows.iter().find(|t| t.target_id == p.id) else { continue };
        if p.number == 1 && !t.title.trim().is_empty() {
            title.clone_from(&t.title);
        }
        let label = if t.machine {
            trh("Translated automatically. Show the original")
        } else {
            match t.translator.as_deref() {
                Some(n) => crate::http::i18n::trf("Translated by {name}. Show the original", &[("name", &esc(n))]),
                None => trh("Translated. Show the original"),
            }
        };
        p.cooked = format!(
            "<div class=\"bf-translation\">{}</div><details class=\"bf-original bf-disclosure is-small small\"><summary>{label}</summary><div>{}</div></details>",
            t.cooked, p.cooked
        );
    }
}

/// What is being translated: (title, source text, where it lives, may this viewer translate).
async fn source(state: &AppState, v: &Viewer, kind: &str, id: i64) -> Result<(String, String, String, bool), AppError> {
    if !enabled(state) {
        return Err(AppError::NotFound);
    }
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    // `translator_groups`: group ids separated by commas.
    let groups = bf_core::settings::group_ids(state.settings().str("translator_groups"));
    let translator = is_staff(v) || groups.iter().any(|g| v.actor.groups.contains(&(*g).into()));
    match kind {
        "post" => {
            #[allow(clippy::type_complexity)]
            let row: Option<(i64, i32, Option<i64>, String, i64, String, String)> = sqlx::query_as(
                "SELECT p.item_id, p.post_number, p.author_id, p.raw, i.node_id, i.title, i.slug FROM posts p
                 JOIN items i ON i.id = p.item_id WHERE p.id = $1 AND p.state = 'visible' AND i.state = 'visible'",
            )
            .bind(id)
            .fetch_optional(pool)
            .await
            .db()?;
            let (item, number, author, raw, node, title, slug) = row.ok_or(AppError::NotFound)?;
            if !tree.perms(&v.actor, node).has(Perms::READ) {
                return Err(AppError::NotFound);
            }
            let own = author == Some(user.id.get());
            let moderator = tree.perms(&v.actor, node).has(Perms::MODERATE);
            // A translation is shown in place of the post, so its author may write one only while
            // they could edit the post itself: not silenced, the topic open, the post not locked,
            // and inside the edit window.
            let may = translator
                || moderator
                || (own && crate::forum::posting::editable_post(state, v, id).await.is_ok());
            // The source is the post's text: `[hide]` blocks only for those the topic shows them to.
            let sees_hidden = moderator
                || own
                || bf_db::community::onboarding::has_posted(pool, item, user.id.get()).await.db()?;
            let raw = if sees_hidden { raw } else { bf_cook::strip_hidden(&raw) };
            let heading = if number == 1 { title } else { String::new() };
            Ok((heading, raw, format!("{}#post-{number}", topic_path(item, &slug)), may))
        }
        _ => Err(AppError::NotFound),
    }
}

#[derive(Deserialize, Default)]
struct PageQuery {
    #[serde(default)]
    locale: String,
}

async fn page(
    State(state): State<AppState>,
    v: Viewer,
    Path((kind, id)): Path<(String, i64)>,
    Query(q): Query<PageQuery>,
) -> Result<Response, AppError> {
    let (title, raw, back, may) = source(&state, &v, &kind, id).await?;
    if !may {
        return Err(AppError::Forbidden);
    }
    let existing = db::of(state.db()?, &kind, id).await.db()?;
    let ctx = begin(&state, &v);
    let locales = bf_core::i18n::locales();
    let name_of = |code: &str| locales.iter().find(|(c, _)| *c == code).map_or_else(|| code.to_owned(), |(_, n)| (*n).to_owned());
    // The language being edited: the one asked for, else the newest version, else the first.
    let chosen = if bf_core::i18n::supported(&q.locale) {
        q.locale.clone()
    } else {
        existing.first().map_or_else(|| locales.first().map(|(c, _)| (*c).to_owned()).unwrap_or_default(), |t| t.locale.clone())
    };
    let current = existing.iter().find(|t| t.locale == chosen);

    // Versions: one row per language, with edit and delete.
    let mut rows = String::new();
    for t in &existing {
        let who = match (t.machine, t.translator.as_deref()) {
            (true, _) => format!("<span class=\"bf-badge\">{}</span>", trh("Automatic")),
            (false, name) => user_link(name),
        };
        rows.push_str(&format!(
            "<li class=\"bf-tr-version{cur}\"><a class=\"bf-tr-lang\" href=\"/translate/{kind}/{id}?locale={loc}\">{lang}</a><span class=\"bf-tr-meta small muted\">{who} · {when}</span><form method=\"post\" action=\"/translate/{kind}/{id}/{loc}/delete\" data-confirm=\"Delete this version?\">{csrf}<button class=\"bf-icon-btn\" aria-label=\"{del}\">{icon}</button></form></li>",
            cur = if t.locale == chosen { " is-current" } else { "" },
            loc = esc(&t.locale),
            lang = esc(&name_of(&t.locale)),
            when = when(t.updated_at),
            csrf = hidden_csrf(&ctx.token),
            del = trh("Delete"),
            icon = ctx.icon("trash-2"),
        ));
    }
    let versions = if existing.is_empty() {
        format!("<p class=\"bf-tr-empty muted\">{}</p>", trh("No translations yet."))
    } else {
        format!("<ul class=\"bf-tr-versions\">{rows}</ul>")
    };

    let mut options = String::new();
    for (code, name) in &locales {
        options.push_str(&format!(
            "<option value=\"{code}\"{}>{}</option>",
            if *code == chosen { " selected" } else { "" },
            esc(name)
        ));
    }
    let editor = format!(
        "<form method=\"post\" action=\"/translate/{kind}/{id}\" class=\"bf-box bf-tr-editor\">{csrf}         <div class=\"bf-tr-fields\">         <label class=\"bf-field\"><select name=\"locale\">{options}</select><span class=\"bf-field-label\">{l1}</span></label>{title_field}{body}         </div>         <div class=\"bf-tr-actions\"><a class=\"btn\" href=\"{back_e}\">{cancel}</a><button class=\"btn primary\">{save}</button></div></form>",
        csrf = hidden_csrf(&ctx.token),
        l1 = trh("Language (saving one that exists replaces it)"),
        title_field = if title.is_empty() { String::new() } else { text_field("title", "Title", current.map_or(title.as_str(), |t| t.title.as_str()), 200, true) },
        body = md_field("raw", "Text (Markdown)", current.map_or(raw.as_str(), |t| t.raw.as_str()), 12, kind == "post"),
        back_e = esc(&back),
        cancel = trh("Cancel"),
        save = trh("Save translation"),
    );

    let original = format!(
        "<section class=\"bf-box bf-tr-original\"><div class=\"bf-box-head\"><h2>{}</h2></div><div class=\"bf-tr-source\">{}{}</div></section>",
        trh("The original"),
        if title.is_empty() { String::new() } else { format!("<p class=\"bf-tr-source-title\">{}</p>", esc(&title)) },
        if raw.trim().is_empty() { format!("<p class=\"muted\">{}</p>", trh("No description.")) } else { format!("<pre class=\"bf-held-text\">{}</pre>", esc(&raw)) },
    );
    let html = format!(
        "<div class=\"bf-tr\">{editor}<aside class=\"bf-tr-side\"><section class=\"bf-box\"><div class=\"bf-box-head\"><h2>{vh}</h2></div>{versions}</section>{original}</aside></div>",
        vh = trh("Versions"),
    );
    let shell = Shell {
        title: v.t().msg("Translations"),
        crumbs: vec![(back, if title.is_empty() { "Back".into() } else { title })],
        noindex: true,
        ..Shell::default()
    };
    Ok(finish(&state, &v, ctx, shell, html))
}

#[derive(Deserialize)]
struct SaveForm {
    csrf: String,
    locale: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    raw: String,
}

async fn save(
    State(state): State<AppState>,
    v: Viewer,
    Path((kind, id)): Path<(String, i64)>,
    Form(f): Form<SaveForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let (title, _, back, may) = source(&state, &v, &kind, id).await?;
    if !may {
        return Err(AppError::Forbidden);
    }
    let user = v.require_user()?;
    if !bf_core::i18n::supported(&f.locale) {
        return Err(AppError::Validation("Choose a language.".into()));
    }
    let text = f.raw.trim();
    if text.chars().count() > 60_000 {
        return Err(AppError::Validation("That text is too long.".into()));
    }
    let t = f.title.trim();
    // Screened as an edit of the post would be: the sites the administrator listed, then the
    // plugins' filters, watched words and the link limit. Staff skip the list, as in posts.
    let mut held = false;
    if !is_staff(&v) {
        let (bad_domain, review_domain) = crate::moderation::spam::link_verdict(&state, &format!("{t}\n{text}"));
        if let Some(domain) = bad_domain {
            return Err(AppError::invalid("Links to {domain} are not allowed here.", &[("domain", &domain)]));
        }
        held = review_domain;
    }
    let (raw, cooked) = if text.is_empty() {
        (String::new(), String::new())
    } else {
        let (checked, cooked) = crate::moderation::gate::check_post(&state, &v, text).await?;
        held |= checked.held;
        (checked.text, cooked.html)
    };
    if held || (!t.is_empty() && crate::moderation::gate::words(&state, &v, t).await?.held) {
        return Err(AppError::Validation("That text needs a moderator; please choose other words.".into()));
    }
    if kind == "post" && raw.is_empty() {
        return Err(AppError::Validation("Write the translation first.".into()));
    }
    let new_title: String = if title.is_empty() { String::new() } else { t.chars().take(200).collect() };
    db::save(state.db()?, &kind, id, &f.locale, &new_title, &raw, &cooked, user.id.get()).await.db()?;
    load(&state).await;
    state.microcache.clear();
    Ok(Redirect::to(&back).into_response())
}

async fn delete(
    State(state): State<AppState>,
    v: Viewer,
    Path((kind, id, locale)): Path<(String, i64, String)>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let (_, _, _, may) = source(&state, &v, &kind, id).await?;
    if !may {
        return Err(AppError::Forbidden);
    }
    db::delete(state.db()?, &kind, id, &locale).await.db()?;
    load(&state).await;
    state.microcache.clear();
    Ok(Redirect::to(&format!("/translate/{kind}/{id}")).into_response())
}
