//! Sends queued mail from `mail_outbox` every few seconds.
//!
//! Transport, by environment:
//! - `BF_SMTP_URL` (e.g. `smtps://user:pass@smtp.example.com`) with `BF_MAIL_FROM`: SMTP.
//! - otherwise `BF_MAIL_DIR`: each message is written to a `.eml` file (development, tests).
//! - otherwise: messages are logged and marked sent, so the queue never grows.

use lettre::message::Mailbox;
use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use sqlx::PgPool;
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

/// The message as it goes over SMTP. The body is declared as UTF-8: with no `Content-Type` a mail
/// client may read it as ASCII and show every letter outside it (`ą`, `ł`, Arabic) as `?`.
fn message(from: &Mailbox, m: &bf_db::platform::mail::OutMail) -> Result<Message, String> {
    let to: Mailbox = m.to_addr.parse().map_err(|e| format!("{BAD_ADDRESS}: {e}"))?;
    let mut b = Message::builder().from(from.clone()).to(to).subject(&m.subject).header(ContentType::TEXT_PLAIN);
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
    b.body(m.body.clone()).map_err(|e| e.to_string())
}

async fn send(t: &Transport, m: &bf_db::platform::mail::OutMail) -> Result<(), String> {
    match t {
        Transport::Smtp(smtp, from) => smtp.send(message(from, m)?).await.map(|_| ()).map_err(|e| e.to_string()),
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

pub fn spawn(pool: PgPool) {
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
                let result = tokio::time::timeout(SEND_TIMEOUT, send(&t, &m))
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
            };
            let raw = String::from_utf8(message(&from, &m).unwrap().formatted()).unwrap();
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
