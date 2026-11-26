//! The Mail page's preview: sample mails made from the same catalog texts and layout members
//! get, shown in a frame of their own, or sent to the administrator to try in a real client.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::i18n::T;
use bf_core::settings::Settings;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/mail/preview", get(text).post(send))
        .route("/api/v1/admin/mail/preview/page", get(page))
}

/// The mails that can be previewed.
const KINDS: [&str; 6] = ["reply", "digest", "login", "security", "verify", "reset"];

#[derive(Deserialize)]
struct Pick {
    kind: String,
    #[serde(default)]
    lang: String,
    /// Show the dark colours, whatever the viewer's own setting.
    #[serde(default)]
    dark: bool,
}

#[derive(Default)]
struct Sample {
    subject: String,
    body: String,
    unsubscribe: Option<String>,
    /// The digest's cards.
    summary: Option<bf_web::mail::Summary>,
}

fn language(state: &AppState, asked: &str) -> String {
    if bf_core::i18n::supported(asked) {
        asked.to_owned()
    } else {
        bf_core::i18n::site_language(state.settings().str("default_locale")).to_owned()
    }
}

/// A week of made-up topics for the summary.
fn sample_summary(base: &str) -> bf_web::mail::Summary {
    let card = |id: u32, forum: &str, title: &str, author: &str, excerpt: &[&str], likes, replies, people: &[&str]| {
        bf_web::mail::Card {
            forum: forum.to_owned(),
            title: title.to_owned(),
            url: format!("{base}/t/sample/{id}"),
            created: chrono::Utc::now() - chrono::Duration::days(i64::from(id)),
            author: author.to_owned(),
            excerpt: excerpt.iter().map(|p| (*p).to_owned()).collect(),
            image: (id == 1).then(|| SAMPLE_PICTURE_PATH.to_owned()),
            likes,
            replies,
            participants: people.iter().map(|p| (*p).to_owned()).collect(),
        }
    };
    bf_web::mail::Summary {
        new_topics: 87,
        unread: 3,
        likes: 22,
        cards: vec![
            card(
                1,
                "Minecraft",
                "Show your survival base",
                "Stephen",
                &["Three weeks in, and the harbour is finally finished.", "Post yours below, screenshots welcome."],
                28,
                10,
                &["scot", "marta", "piotr_k", "ola"],
            ),
            card(
                2,
                "Counter-Strike",
                "Looking for a team for the weekend cup",
                "scot",
                &["We need one more player. Evenings, voice chat, no pressure."],
                26,
                13,
                &["andrey", "ola"],
            ),
            card(
                3,
                "Guides",
                "Server settings that stopped our lag",
                "piotr_k",
                &["I would start with a view distance of 8. On 4 GB of memory that stopped the lag spikes for us…"],
                4,
                11,
                &["marta", "Stephen"],
            ),
        ],
        more: vec![bf_web::mail::Line {
            title: "Which launcher do you use?".to_owned(),
            url: format!("{base}/t/sample/4"),
            replies: 5,
        }],
        ..bf_web::mail::Summary::default()
    }
}

/// The path the sample card names for its picture; the preview draws one of its own.
const SAMPLE_PICTURE_PATH: &str = "/uploads/sample-picture";

/// A small made-up thumbnail, drawn here so the preview shows where pictures go.
fn sample_picture() -> Option<String> {
    use base64::Engine;
    let picture = image::RgbImage::from_fn(160, 160, |x, y| {
        let hill = 100.0 + 22.0 * (f64::from(x) / 26.0).sin();
        if f64::from(y) > hill { image::Rgb([70, 130, 80]) } else { image::Rgb([150, 190, (230 - y / 3) as u8]) }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(out.into_inner())))
}

/// A mail of `kind` with made-up names, built the way the real one is.
fn sample(settings: &Settings, base_url: &str, kind: &str, lang: &str) -> Result<Sample, AppError> {
    let t = T::new(lang);
    let site = settings.str("site_name");
    let base = base_url.trim_end_matches('/');
    let unsubscribe = format!("{base}/unsubscribe/sample");
    let render = |kind: &str, vars: &[(&str, &str)]| crate::admin::mail_templates::render(&t, kind, vars);
    let user = ("username", "Marta");
    let site_var = ("site", site);
    let made = match kind {
        "reply" => {
            let link = format!("{base}/t/sample/1#post-2");
            let title = "Best server settings?";
            let vars = [site_var, user, ("by", "piotr_k"), ("title", title), ("link", link.as_str())];
            let (subject, body) = render("notification", &vars);
            let quote =
                "I would start with a view distance of 8. On 4 GB of memory that stopped the lag spikes for us…";
            let mut body = bf_web::mail::with_quote(&body, &link, "piotr_k", quote);
            // The footer notification mail gets as it is queued.
            body.push_str(&format!(
                "\n{} {unsubscribe}\n{} {unsubscribe}/topic",
                t.msg("To stop these emails, open:"),
                t.msg("To mute this topic, open:")
            ));
            Sample { subject, body, unsubscribe: Some(unsubscribe), ..Sample::default() }
        }
        "digest" => {
            let summary = sample_summary(base);
            let listed: String = summary
                .cards
                .iter()
                .map(|c| (c.title.as_str(), c.url.as_str(), c.replies))
                .chain(summary.more.iter().map(|l| (l.title.as_str(), l.url.as_str(), l.replies)))
                .map(|(title, url, replies)| format!("- {title} ({})\n  {url}\n", t.n("count.replies", replies)))
                .collect();
            let topics = format!(
                "{}\n\n{listed}\n{}\n{base}/notifications\n",
                t.msg("New topics since your last digest:"),
                t.msgf("You have {count} unread notifications.", &[("count", "3")]),
            );
            let vars = [site_var, user, ("topics", topics.as_str()), ("link", base), ("unsubscribe", &unsubscribe)];
            let (subject, body) = render("digest", &vars);
            Sample { subject, body, unsubscribe: Some(unsubscribe), summary: Some(summary) }
        }
        "login" => {
            let (subject, body) =
                crate::auth::login::new_login_mail(&t, "Marta", "203.0.113.24", site, chrono::Utc::now(), base);
            Sample { subject, body, ..Sample::default() }
        }
        "security" => {
            let forgot = format!("{base}/forgot");
            let what = t.msg("Your password was changed");
            let (subject, body) = render("security", &[site_var, user, ("what", &what), ("link", &forgot)]);
            Sample { subject, body, ..Sample::default() }
        }
        "verify" | "reset" => {
            let link = format!("{base}/{kind}/sample-token");
            let (subject, body) = render(kind, &[site_var, user, ("link", &link)]);
            Sample { subject, body, ..Sample::default() }
        }
        _ => return Err(AppError::NotFound),
    };
    Ok(made)
}

/// `GET /api/v1/admin/mail/preview`: the subject and plain text.
async fn text(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Query(q): Query<Pick>,
) -> Result<Response, AppError> {
    let s = sample(&state.settings(), &state.config.base_url, &q.kind, &language(&state, &q.lang))?;
    Ok(Json(json!({ "subject": s.subject, "text": s.body, "kinds": KINDS })).into_response())
}

/// `GET /api/v1/admin/mail/preview/page`: the HTML version, as a page of its own for a frame.
/// A mail styles itself inline, which the panel's policy forbids, so it gets its own: no
/// scripts, nothing loaded from anywhere, framed only by this site.
async fn page(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Query(q): Query<Pick>,
) -> Result<Response, AppError> {
    let lang = language(&state, &q.lang);
    let s = sample(&state.settings(), &state.config.base_url, &q.kind, &lang)?;
    let logo = crate::platform::mail::logo(&state).await;
    let settings = state.settings();
    let brand = crate::platform::mail::brand(
        &state.config.base_url,
        &settings,
        logo.as_ref().map(crate::platform::mail::Logo::as_data_url),
    );
    let mail = bf_web::mail::Mail { subject: &s.subject, body: &s.body, unsubscribe_url: s.unsubscribe.as_deref() };
    let html = match &s.summary {
        Some(summary) => {
            let mut pictures = bf_web::mail::Pictures::default();
            if let Some(url) = sample_picture() {
                pictures.images.insert(SAMPLE_PICTURE_PATH.to_owned(), url);
            }
            bf_web::mail::render_summary(&brand, &mail, summary, &pictures, &lang, q.dark)
        }
        None => bf_web::mail::render(&brand, &mail, &lang, q.dark),
    }
    .map_err(AppError::internal)?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; img-src data:; sandbox; frame-ancestors 'self'",
            ),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        html,
    )
        .into_response())
}

#[derive(Deserialize)]
struct Send {
    kind: String,
    #[serde(default)]
    lang: String,
}

/// `POST /api/v1/admin/mail/preview`: queues the sample to the signed-in administrator's own
/// address, and nowhere else.
async fn send(
    State(state): State<AppState>,
    Admin(_, v): AdminWrite,
    Json(q): Json<Send>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let pool = state.db()?;
    let account = bf_db::accounts::users::user_by_id(pool, user.id).await.db()?.ok_or(AppError::NotFound)?;
    let s = sample(&state.settings(), &state.config.base_url, &q.kind, &language(&state, &q.lang))?;
    let email: String =
        sqlx::query_scalar("SELECT email FROM users WHERE id = $1").bind(account.id).fetch_one(pool).await.db()?;
    match (&s.summary, &s.unsubscribe) {
        (Some(summary), Some(unsubscribe)) => {
            let summary = serde_json::to_value(summary).map_err(AppError::internal)?;
            bf_db::platform::digests::mark_digest_sent(
                pool,
                &email,
                &s.subject,
                Some(&s.body),
                unsubscribe,
                Some(&summary),
            )
            .await
            .db()?;
        }
        _ => bf_db::platform::mail::queue_mail_with_unsubscribe(
            pool,
            &email,
            &s.subject,
            &s.body,
            s.unsubscribe.as_deref(),
        )
        .await
        .db()?,
    }
    Ok((StatusCode::OK, Json(json!({ "to": email }))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sample_is_a_whole_mail_in_every_language() {
        let settings = Settings::default();
        let base = "https://f.test";
        for (lang, _) in bf_core::i18n::locales() {
            for kind in KINDS {
                let s = sample(&settings, base, kind, lang).unwrap();
                assert!(!s.subject.is_empty() && !s.body.contains("{ $"), "{lang} {kind}: {}", s.body);
                let brand = crate::platform::mail::brand(base, &settings, None);
                let mail = bf_web::mail::Mail {
                    subject: &s.subject,
                    body: &s.body,
                    unsubscribe_url: s.unsubscribe.as_deref(),
                };
                if let Some(summary) = &s.summary {
                    let pictures = bf_web::mail::Pictures::default();
                    let html = bf_web::mail::render_summary(&brand, &mail, summary, &pictures, lang, false).unwrap();
                    assert!(html.contains("/t/sample/1\"") && html.contains("/unsubscribe/sample\""), "{lang}");
                    // The text's list of topics became the cards.
                    assert!(!html.contains("- Show your survival base"), "{lang}\n{html}");
                    continue;
                }
                let html = bf_web::mail::render(&brand, &mail, lang, false).unwrap();
                assert!(html.contains("<h1"), "{lang} {kind}");
                // This one links from inside a sentence.
                if kind != "security" {
                    assert!(html.contains("class=\"m-btn\""), "{lang} {kind} has no button\n{}", s.body);
                }
                assert_eq!(s.unsubscribe.is_some(), html.contains("/unsubscribe/sample\""), "{lang} {kind}");
            }
        }
        assert!(sample(&settings, base, "nope", "en").is_err());
    }
}
