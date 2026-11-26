//! Sends queued mail from `mail_outbox` every few seconds.
//!
//! Transport, by environment:
//! - `BF_SMTP_URL` (e.g. `smtps://user:pass@smtp.example.com`) with `BF_MAIL_FROM`: SMTP.
//! - otherwise `BF_MAIL_DIR`: each message is written to a `.eml` file (development, tests).
//! - otherwise: messages are logged and marked sent, so the queue never grows.

use crate::state::AppState;
use base64::Engine;
use lettre::message::Mailbox;
use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::{Attachment, MultiPart, SinglePart};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

enum Transport {
    Smtp(Box<AsyncSmtpTransport<Tokio1Executor>>, Mailbox),
    Dir(PathBuf),
    Log,
}

/// What the sender is doing, for the admin panel's Mail page: `smtp`, `dir` (files, for development),
/// `log` (nothing is delivered), `error` (misconfigured: mail stays queued) or `starting`.
#[derive(Clone, serde::Serialize)]
pub struct SenderStatus {
    pub kind: &'static str,
    pub detail: String,
}

static STATUS: LazyLock<RwLock<SenderStatus>> =
    LazyLock::new(|| RwLock::new(SenderStatus { kind: "starting", detail: String::new() }));

pub fn sender_status() -> SenderStatus {
    STATUS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

fn set_status(kind: &'static str, detail: String) {
    *STATUS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = SenderStatus { kind, detail };
}

fn transport() -> Result<Transport, String> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let Some(url) = var("BF_SMTP_URL") {
        let from: Mailbox = var("BF_MAIL_FROM")
            .ok_or("BF_MAIL_FROM is required with BF_SMTP_URL")?
            .parse()
            .map_err(|e| format!("BF_MAIL_FROM: {e}"))?;
        let smtp =
            AsyncSmtpTransport::<Tokio1Executor>::from_url(&url).map_err(|e| format!("BF_SMTP_URL: {e}"))?.build();
        return Ok(Transport::Smtp(Box::new(smtp), from));
    }
    if let Some(dir) = var("BF_MAIL_DIR") {
        std::fs::create_dir_all(&dir).map_err(|e| format!("BF_MAIL_DIR: {e}"))?;
        return Ok(Transport::Dir(dir.into()));
    }
    Ok(Transport::Log)
}

/// How the error of a recipient address that cannot be read starts.
const BAD_ADDRESS: &str = "bad address";

/// The site's logo made ready for mail: a PNG at twice the size it is shown.
pub(crate) struct Logo {
    data_url: String,
    width: u32,
    height: u32,
}

impl Logo {
    /// For a preview: the picture inside the page.
    pub(crate) fn as_data_url(&self) -> (&str, u32, u32) {
        (&self.data_url, self.width, self.height)
    }

    /// For a sent mail: the picture travels with it, so nothing is fetched when it is opened.
    fn attached(&self) -> (&'static str, u32, u32) {
        ("cid:logo", self.width, self.height)
    }
}

/// The email logo from the settings (`mail_logo_url`, else the site's), if it can be drawn. It
/// is shown at most 220 by 40.
pub(crate) async fn logo(state: &AppState) -> Option<Logo> {
    let settings = state.settings();
    let path = [settings.str("mail_logo_url"), settings.str("logo_url")].into_iter().find(|p| !p.is_empty())?;
    let (data_url, width, height) = crate::platform::og::png_logo(path, (440, 80)).await?;
    Some(Logo { data_url, width: (width / 2).max(1), height: (height / 2).max(1) })
}

pub(crate) fn brand<'a>(
    base_url: &'a str,
    settings: &'a bf_core::settings::Settings,
    logo: Option<(&'a str, u32, u32)>,
) -> bf_web::mail::Brand<'a> {
    bf_web::mail::Brand {
        site: settings.str("site_name"),
        base: base_url.trim_end_matches('/'),
        accent: settings.str("mail_accent_colour"),
        logo,
    }
}

/// The HTML version of a queued mail, with the logo that goes with it.
struct Html {
    page: String,
    pictures: Vec<Attached>,
}

/// A picture inside the mail, which the HTML names as `cid:{id}`.
struct Attached {
    id: String,
    kind: &'static str,
    bytes: Vec<u8>,
}

impl Attached {
    /// From a `data:` URL of one of the two kinds the pictures are made in.
    fn from_data_url(id: String, data_url: &str) -> Option<Self> {
        let (kind, data) = ["image/png", "image/jpeg"]
            .into_iter()
            .find_map(|k| data_url.strip_prefix(&format!("data:{k};base64,")).map(|d| (k, d)))?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
        Some(Self { id, kind, bytes })
    }
}

/// The most faces one mail carries; past it, members show as their initial.
const MAX_FACES: usize = 40;

/// The faces and thumbnails of a summary, attached to the mail: each is named `cid:` in the HTML.
async fn summary_pictures(
    state: &AppState,
    summary: &bf_web::mail::Summary,
    attached: &mut Vec<Attached>,
) -> bf_web::mail::Pictures {
    let mut pictures = bf_web::mail::Pictures::default();
    let mut names: Vec<&str> = summary
        .cards
        .iter()
        .flat_map(|c| std::iter::once(&c.author).chain(&c.participants))
        .map(String::as_str)
        .filter(|n| !n.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    for (i, name) in names.into_iter().take(MAX_FACES).enumerate() {
        let Some(pool) = &state.pool else { break };
        let Ok(Some(key)) = bf_db::forum::uploads::avatar_key(pool, name).await else { continue };
        let Some((data_url, ..)) = crate::platform::og::png_logo(&format!("/uploads/{key}"), (72, 72)).await else {
            continue;
        };
        if let Some(picture) = Attached::from_data_url(format!("face{i}"), &data_url) {
            pictures.avatars.insert(name.to_owned(), format!("cid:{}", picture.id));
            attached.push(picture);
        }
    }
    for (i, path) in summary.cards.iter().filter_map(|c| c.image.as_deref()).enumerate() {
        let Some(data_url) = crate::platform::og::square_jpeg(path, 160).await else { continue };
        if let Some(picture) = Attached::from_data_url(format!("thumb{i}"), &data_url) {
            pictures.images.insert(path.to_owned(), format!("cid:{}", picture.id));
            attached.push(picture);
        }
    }
    pictures
}

/// Made as the mail is sent, from its text, in the recipient's language. A mail whose HTML cannot
/// be made still goes out as text.
async fn html_for(state: &AppState, m: &bf_db::platform::mail::OutMail) -> Option<Html> {
    let settings = state.settings();
    let chosen = match &state.pool {
        Some(pool) => bf_db::platform::mail::recipient_locale(pool, &m.to_addr).await.ok().flatten(),
        None => None,
    };
    let lang = chosen.unwrap_or_else(|| bf_core::i18n::site_language(settings.str("default_locale")).to_owned());
    let logo = logo(state).await;
    let mut attached = Vec::new();
    attached.extend(logo.as_ref().and_then(|l| Attached::from_data_url("logo".to_owned(), &l.data_url)));
    let shown = logo.as_ref().filter(|_| !attached.is_empty()).map(Logo::attached);
    let mail = bf_web::mail::Mail { subject: &m.subject, body: &m.body, unsubscribe_url: m.unsubscribe_url.as_deref() };
    let brand = brand(&state.config.base_url, &settings, shown);
    // A digest queued with its cards gets the summary layout; any other mail, the plain one.
    let summary = m.summary.clone().and_then(|v| serde_json::from_value::<bf_web::mail::Summary>(v).ok());
    let made = match summary {
        Some(summary) => {
            let pictures = summary_pictures(state, &summary, &mut attached).await;
            bf_web::mail::render_summary(&brand, &mail, &summary, &pictures, &lang, false)
        }
        None => bf_web::mail::render(&brand, &mail, &lang, false),
    };
    match made {
        Ok(page) => Some(Html { page, pictures: attached }),
        Err(e) => {
            tracing::warn!(error = %e, id = m.id, "mail HTML not made; sending text only");
            None
        }
    }
}

/// The text and, beside it, the HTML with its pictures. A client shows the one it prefers.
fn both_versions(text: String, html: &Html) -> Result<MultiPart, String> {
    let text = SinglePart::plain(text);
    let page = SinglePart::html(html.page.clone());
    if html.pictures.is_empty() {
        return Ok(MultiPart::alternative().singlepart(text).singlepart(page));
    }
    let mut related = MultiPart::related().singlepart(page);
    for picture in &html.pictures {
        let kind = ContentType::parse(picture.kind).map_err(|e| e.to_string())?;
        related = related.singlepart(Attachment::new_inline(picture.id.clone()).body(picture.bytes.clone(), kind));
    }
    Ok(MultiPart::alternative().singlepart(text).multipart(related))
}

/// The message as it goes over SMTP. Every part is declared as UTF-8: with no `Content-Type` a
/// mail client may read it as ASCII and show every letter outside it (`ą`, `ł`, Arabic) as `?`.
fn message(from: &Mailbox, m: &bf_db::platform::mail::OutMail, html: Option<&Html>) -> Result<Message, String> {
    let to: Mailbox = m.to_addr.parse().map_err(|e| format!("{BAD_ADDRESS}: {e}"))?;
    let mut b = Message::builder().from(from.clone()).to(to).subject(&m.subject);
    // Tells autoresponders not to answer (RFC 3834).
    let h = |n: &'static str, v: String| HeaderValue::new(HeaderName::new_from_ascii_str(n), v);
    b = b.raw_header(h("Auto-Submitted", "auto-generated".into()));
    if let Some(reply) = reply_address(m).and_then(|a| a.parse::<Mailbox>().ok()) {
        b = b.reply_to(reply);
    }
    if let Some(u) = &m.unsubscribe_url {
        b = b.raw_header(h("List-Unsubscribe", format!("<{u}>")));
        b = b.raw_header(h("List-Unsubscribe-Post", "List-Unsubscribe=One-Click".into()));
    }
    match html {
        Some(html) => b.multipart(both_versions(m.body.clone(), html)?),
        None => b.header(ContentType::TEXT_PLAIN).body(m.body.clone()),
    }
    .map_err(|e| e.to_string())
}

async fn send(state: &AppState, t: &Transport, m: &bf_db::platform::mail::OutMail) -> Result<(), String> {
    match t {
        Transport::Smtp(smtp, from) => {
            let html = html_for(state, m).await;
            smtp.send(message(from, m, html.as_ref())?).await.map(|_| ()).map_err(|e| e.to_string())
        }
        Transport::Dir(dir) => {
            let extra = m
                .unsubscribe_url
                .as_ref()
                .map(|u| format!("List-Unsubscribe: <{u}>\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\n"))
                .unwrap_or_default();
            let reply = reply_address(m).map(|a| format!("Reply-To: {a}\n")).unwrap_or_default();
            let text = format!(
                "To: {}\nSubject: {}\nContent-Type: text/plain; charset=utf-8\n{reply}{extra}\n{}",
                m.to_addr, m.subject, m.body
            );
            // The HTML version beside it, to look at in a browser.
            if let Some(html) = html_for(state, m).await {
                let page = dir.join(format!("{:08}.html", m.id));
                tokio::fs::write(page, html.page).await.map_err(|e| e.to_string())?;
            }
            tokio::fs::write(dir.join(format!("{:08}.eml", m.id)), text).await.map_err(|e| e.to_string())
        }
        Transport::Log => {
            tracing::info!(to = %m.to_addr, subject = %m.subject, "mail (no transport configured; not delivered)");
            Ok(())
        }
    }
}

/// `reply+KEY@BF_REPLY_DOMAIN`, when replies by email are set up.
fn reply_address(m: &bf_db::platform::mail::OutMail) -> Option<String> {
    let domain = std::env::var("BF_REPLY_DOMAIN").ok().filter(|d| !d.is_empty())?;
    m.reply_key.as_ref().map(|k| format!("reply+{k}@{domain}"))
}

/// One message's time limit. The lease on a claimed batch of 20 (15 minutes) outlasts 20 sends
/// that all take this long, so a slow server cannot get a batch re-claimed and sent twice.
const SEND_TIMEOUT: Duration = Duration::from_secs(40);

pub fn spawn(state: AppState) {
    let Some(pool) = state.pool.clone() else { return };
    let t = match transport() {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "mail transport misconfigured; mail stays queued");
            set_status("error", e);
            return;
        }
    };
    match &t {
        Transport::Smtp(..) => set_status("smtp", String::new()),
        Transport::Dir(dir) => set_status("dir", dir.display().to_string()),
        Transport::Log => set_status("log", String::new()),
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(bf_core::scaled(Duration::from_secs(2)));
        loop {
            tick.tick().await;
            if let Err(e) = bf_db::platform::mail::skip_mail_to_bad_addresses(&pool).await {
                tracing::warn!(error = %e, "skipping bad addresses failed");
            }
            let batch = match bf_db::platform::mail::unsent_mail(&pool, 20).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "mail queue read failed");
                    continue;
                }
            };
            for m in batch {
                let result = tokio::time::timeout(SEND_TIMEOUT, send(&state, &t, &m))
                    .await
                    .unwrap_or_else(|_| Err("timed out sending".into()));
                let saved = match &result {
                    Ok(()) => bf_db::platform::mail::mail_sent(&pool, m.id).await,
                    Err(e) => {
                        tracing::warn!(id = m.id, attempt = m.attempts + 1, error = %e, "mail send failed");
                        // An address that cannot be read never will be. Everything else is tried again, also a
                        // refusal the server calls permanent: a wrong SMTP password is one.
                        bf_db::platform::mail::mail_failed(&pool, m.id, e, e.starts_with(BAD_ADDRESS)).await
                    }
                };
                if let Err(e) = saved {
                    tracing::warn!(error = %e, "mail status update failed");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branded_mail_carries_text_html_and_its_logo() {
        let from: Mailbox = "Forum <forum@example.com>".parse().unwrap();
        let m = bf_db::platform::mail::OutMail {
            id: 1,
            to_addr: "member@example.com".into(),
            subject: "Hello".into(),
            body: "Zażółć".into(),
            attempts: 0,
            unsubscribe_url: Some("https://f.test/unsubscribe/x".into()),
            reply_key: None,
            summary: None,
        };
        let logo = Attached::from_data_url("logo".into(), "data:image/png;base64,iVBORw==").unwrap();
        let face = Attached::from_data_url("face0".into(), "data:image/jpeg;base64,/9j/").unwrap();
        assert!(Attached::from_data_url("x".into(), "data:image/svg+xml;base64,PHN2Zy8+").is_none());
        let html = Html { page: "<p>Zażółć</p>".into(), pictures: vec![logo, face] };
        let raw = String::from_utf8(message(&from, &m, Some(&html)).unwrap().formatted()).unwrap();
        for part in [
            "multipart/alternative",
            "multipart/related",
            "Content-Type: text/plain; charset=utf-8",
            "Content-Type: text/html; charset=utf-8",
            "Content-Type: image/png",
            "Content-ID: <logo>",
            "Content-ID: <face0>",
            "Content-Type: image/jpeg",
            "Content-Disposition: inline",
            "List-Unsubscribe: <https://f.test/unsubscribe/x>",
        ] {
            assert!(raw.contains(part), "{part}\n{raw}");
        }
        // The text comes first: a client shows the last version it understands.
        assert!(raw.find("text/plain").unwrap() < raw.find("text/html").unwrap());
    }

    #[test]
    fn mail_declares_utf8_for_polish_and_arabic_text() {
        let from: Mailbox = "Forum Gżegżółka <forum@example.com>".parse().unwrap();
        for body in ["Zażółć gęślą jaźń.\nŁódź, środa.", "مرحبًا بكم في المنتدى", "Only ASCII."]
        {
            let m = bf_db::platform::mail::OutMail {
                id: 1,
                to_addr: "member@example.com".into(),
                subject: "Odpowiedź w wątku".into(),
                body: body.into(),
                attempts: 0,
                unsubscribe_url: None,
                reply_key: None,
                summary: None,
            };
            let raw = String::from_utf8(message(&from, &m, None).unwrap().formatted()).unwrap();
            let (head, sent) = raw.split_once("\r\n\r\n").unwrap();
            assert!(head.contains("Content-Type: text/plain; charset=utf-8"), "{head}");
            assert!(head.contains("Content-Transfer-Encoding: "), "{head}");
            assert!(head.contains("Auto-Submitted: auto-generated"), "{head}");
            // Headers are ASCII: names and the subject travel as encoded words.
            assert!(head.is_ascii() && head.contains("=?utf-8?"), "{head}");
            // Raw bytes outside ASCII go out only when the message says it is 8-bit.
            assert!(sent.is_ascii() || head.contains("Content-Transfer-Encoding: 8bit"), "{head}\n{sent}");
        }
    }
}
