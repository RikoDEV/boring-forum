//! Email-in over IMAP (optional): with `BF_IMAP_HOST`, `BF_IMAP_USER` and `BF_IMAP_PASSWORD` set,
//! the forum polls that mailbox for unread mail and handles each one as the inbound webhook does
//! (`forum::topic_tools::process_inbound`). Without them nothing runs and no connection is made.
//!
//! This is a small client for exactly what polling needs (LOGIN, SELECT, SEARCH, FETCH, STORE)
//! over implicit TLS, plus a reader for the common mail shapes: single part, multipart,
//! quoted-printable and base64.

use crate::state::AppState;
use base64::Engine;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// A mail taken apart.
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub addresses: Vec<String>,
    pub subject: String,
    pub text: String,
    pub html: String,
    /// Files attached to the mail, decoded; the upload pipeline decides what is kept.
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, PartialEq)]
pub struct Attachment {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// More than this many files in one mail is refused by the caller.
const MAX_ATTACHMENTS: usize = 10;

struct Settings {
    host: String,
    port: u16,
    user: String,
    password: String,
    every: Duration,
}

fn settings() -> Option<Settings> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    Some(Settings {
        host: var("BF_IMAP_HOST")?,
        port: var("BF_IMAP_PORT").and_then(|p| p.parse().ok()).unwrap_or(993),
        user: var("BF_IMAP_USER")?,
        password: var("BF_IMAP_PASSWORD")?,
        every: Duration::from_secs(var("BF_IMAP_INTERVAL_SECS").and_then(|p| p.parse().ok()).unwrap_or(60).max(15)),
    })
}

pub fn spawn(state: AppState) {
    let Some(s) = settings() else { return };
    tracing::info!(host = %s.host, "email-in: polling the mailbox over IMAP");
    tokio::spawn(async move {
        loop {
            if state.settings().bool("offline") {
                tokio::time::sleep(s.every).await;
                continue;
            }
            match poll(&state, &s).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(mails = n, "email-in: mails handled"),
                Err(e) => tracing::warn!(error = %e, "email-in: poll failed"),
            }
            tokio::time::sleep(s.every).await;
        }
    });
}

type Stream = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;

struct Imap {
    io: BufReader<Stream>,
    n: u32,
}

impl Imap {
    /// Sends one command and reads up to its tagged answer: the lines (without literals) and
    /// the literals, in order.
    async fn run(&mut self, command: &str) -> Result<(Vec<String>, Vec<Vec<u8>>), String> {
        self.n += 1;
        let tag = format!("a{}", self.n);
        self.io.get_mut().write_all(format!("{tag} {command}\r\n").as_bytes()).await.map_err(|e| e.to_string())?;
        let mut lines = Vec::new();
        let mut literals = Vec::new();
        loop {
            let mut line = String::new();
            let read = self.io.read_line(&mut line).await.map_err(|e| e.to_string())?;
            if read == 0 {
                return Err("connection closed".into());
            }
            let line = line.trim_end_matches(['\r', '\n']).to_owned();
            if let Some(rest) = line.strip_prefix(&format!("{tag} ")) {
                return if rest.starts_with("OK") { Ok((lines, literals)) } else { Err(rest.to_owned()) };
            }
            let size = line.strip_suffix('}').and_then(|l| l.rsplit_once('{')).and_then(|(_, n)| n.parse::<usize>().ok());
            if let Some(size) = size {
                // A mail is never trusted to be small.
                if size > 5_000_000 {
                    return Err("mail too large".into());
                }
                let mut buf = vec![0; size];
                self.io.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
                literals.push(buf);
            }
            lines.push(line);
        }
    }
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

async fn poll(state: &AppState, s: &Settings) -> Result<usize, String> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tcp = tokio::time::timeout(Duration::from_secs(20), tokio::net::TcpStream::connect((s.host.as_str(), s.port)))
        .await
        .map_err(|_| "connect timed out".to_owned())?
        .map_err(|e| e.to_string())?;
    let name = ServerName::try_from(s.host.clone()).map_err(|e| e.to_string())?;
    let tls = TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map_err(|e| e.to_string())?;
    let mut imap = Imap { io: BufReader::new(tls), n: 0 };
    let mut greeting = String::new();
    imap.io.read_line(&mut greeting).await.map_err(|e| e.to_string())?;
    imap.run(&format!("LOGIN {} {}", quoted(&s.user), quoted(&s.password))).await?;
    imap.run("SELECT INBOX").await?;
    let (found, _) = imap.run("SEARCH UNSEEN").await?;
    let ids: Vec<u32> = found
        .iter()
        .filter_map(|l| l.strip_prefix("* SEARCH"))
        .flat_map(|l| l.split_whitespace().filter_map(|n| n.parse().ok()).collect::<Vec<u32>>())
        .take(20)
        .collect();
    let mut handled = 0;
    for id in ids {
        // PEEK leaves it unread until it has been handled; a failure is then retried next time.
        let (_, literals) = imap.run(&format!("FETCH {id} BODY.PEEK[]")).await?;
        if let Some(raw) = literals.first() {
            let mail = parse(raw);
            let files: Vec<(String, Vec<u8>)> = mail.attachments.into_iter().map(|a| (a.filename, a.bytes)).collect();
            crate::forum::topic_tools::process_inbound(
                state,
                &mail.addresses,
                &mail.subject,
                &mail.text,
                &mail.html,
                &files,
            )
            .await;
        }
        imap.run(&format!("STORE {id} +FLAGS (\\Seen)")).await?;
        handled += 1;
    }
    let _ = imap.run("LOGOUT").await;
    Ok(handled)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Splits a message into header lines (unfolded, names in lower case) and body.
fn split(raw: &[u8]) -> (Vec<(String, String)>, &[u8]) {
    let pos = find(raw, b"\r\n\r\n").map(|p| (p, 4)).or_else(|| find(raw, b"\n\n").map(|p| (p, 2)));
    let (head, body) = match pos {
        Some((p, n)) => (&raw[..p], &raw[p + n..]),
        None => (raw, &raw[raw.len()..]),
    };
    let head = String::from_utf8_lossy(head);
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    (headers, body)
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// The value of a `Content-Type` parameter such as `boundary` or `charset`.
fn param(content_type: &str, name: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim().trim_matches('"').to_owned())
    })
}

fn to_string(bytes: &[u8], charset: &str) -> String {
    match charset.to_ascii_lowercase().as_str() {
        "iso-8859-1" | "latin1" | "us-ascii" | "windows-1252" => bytes.iter().map(|b| char::from(*b)).collect(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn quoted_printable(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            b'=' if body.get(i + 1) == Some(&b'\r') && body.get(i + 2) == Some(&b'\n') => i += 3,
            b'=' if body.get(i + 1) == Some(&b'\n') => i += 2,
            b'=' if i + 2 < body.len() => {
                let hex = std::str::from_utf8(&body[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                if let Some(b) = hex {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(b'=');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

fn decode_body(body: &[u8], cte: &str, charset: &str) -> String {
    to_string(&decode_bytes(body, cte), charset)
}

fn decode_bytes(body: &[u8], cte: &str) -> Vec<u8> {
    match cte.trim().to_ascii_lowercase().as_str() {
        "base64" => {
            let clean: Vec<u8> = body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(clean).unwrap_or_default()
        }
        "quoted-printable" => quoted_printable(body),
        _ => body.to_vec(),
    }
}

/// A header value with `=?charset?B|Q?text?=` words read.
fn decode_words(v: &str) -> String {
    let mut out = String::new();
    let mut rest = v;
    while let Some(start) = rest.find("=?") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let parts: Vec<&str> = after.splitn(4, '?').collect();
        if let [charset, enc, text, tail] = parts[..]
            && let Some(tail) = tail.strip_prefix('=')
        {
            let bytes = if enc.eq_ignore_ascii_case("b") {
                base64::engine::general_purpose::STANDARD.decode(text).unwrap_or_default()
            } else {
                quoted_printable(text.replace('_', " ").as_bytes())
            };
            out.push_str(&to_string(&bytes, charset));
            rest = tail;
            // Words separated only by a space belong together.
            rest = rest.strip_prefix(' ').filter(|r| r.starts_with("=?")).unwrap_or(rest);
        } else {
            out.push_str("=?");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Every address in an address header: what stands in `<>`, else the bare word with an `@`.
fn addresses(v: &str) -> Vec<String> {
    v.split(',')
        .filter_map(|part| {
            let a = match (part.find('<'), part.find('>')) {
                (Some(s), Some(e)) if e > s => &part[s + 1..e],
                _ => part,
            };
            let a = a.trim().trim_matches('"');
            a.contains('@').then(|| a.to_owned())
        })
        .collect()
}

fn walk(headers: &[(String, String)], body: &[u8], out: &mut Parsed, depth: u8) {
    let ct = header(headers, "content-type").unwrap_or("text/plain");
    let kind = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let cte = header(headers, "content-transfer-encoding").unwrap_or("7bit");
    if kind.starts_with("multipart/") && depth < 5 {
        let Some(boundary) = param(ct, "boundary") else { return };
        let marker = format!("--{boundary}");
        let text = String::from_utf8_lossy(body);
        for part in text.split(&marker).skip(1) {
            if part.starts_with("--") {
                break;
            }
            let part = part.strip_prefix("\r\n").or_else(|| part.strip_prefix('\n')).unwrap_or(part);
            let (h, b) = split(part.as_bytes());
            walk(&h, b, out, depth + 1);
        }
        return;
    }
    // Attachments are not part of the post text; they go to the upload pipeline.
    let disposition = header(headers, "content-disposition").unwrap_or("");
    let attached = disposition.trim_start().to_ascii_lowercase().starts_with("attachment");
    let filename = param(disposition, "filename").or_else(|| param(ct, "name")).map(|n| decode_words(&n));
    if attached || (filename.is_some() && !kind.starts_with("text/plain") && !kind.starts_with("text/html")) {
        // One more than the limit is kept, so the caller can tell and refuse the mail.
        if out.attachments.len() <= MAX_ATTACHMENTS {
            let bytes = decode_bytes(body, cte);
            out.attachments.push(Attachment { filename: filename.unwrap_or_else(|| "file".into()), bytes });
        }
        return;
    }
    let charset = param(ct, "charset").unwrap_or_else(|| "utf-8".into());
    match kind.as_str() {
        "text/plain" if out.text.is_empty() => out.text = decode_body(body, cte, &charset),
        "text/html" if out.html.is_empty() => out.html = decode_body(body, cte, &charset),
        _ => {}
    }
}

/// Takes a raw message apart: who it was sent to, the subject, and its text and HTML parts.
pub fn parse(raw: &[u8]) -> Parsed {
    let (headers, body) = split(raw);
    let mut out = Parsed { subject: decode_words(header(&headers, "subject").unwrap_or("")), ..Parsed::default() };
    for name in ["delivered-to", "x-original-to", "to", "cc"] {
        for (_, v) in headers.iter().filter(|(k, _)| k == name) {
            out.addresses.extend(addresses(&decode_words(v)));
        }
    }
    walk(&headers, body, &mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_mail_with_encoded_subject() {
        let raw = b"To: Forum <reply+abc@x.example>\r\nSubject: =?UTF-8?B?SGVsbG8=?=\r\n\r\nThanks!\r\n";
        let p = parse(raw);
        assert_eq!(p.addresses, ["reply+abc@x.example"]);
        assert_eq!(p.subject, "Hello");
        assert_eq!(p.text.trim(), "Thanks!");
    }

    #[test]
    fn multipart_keeps_html_and_skips_attachments() {
        let raw = b"To: reply+k@x.example\nContent-Type: multipart/mixed; boundary=\"B\"\n\n--B\nContent-Type: text/html; charset=utf-8\nContent-Transfer-Encoding: quoted-printable\n\n<p>Caf=C3=A9</p>\n--B\nContent-Type: application/pdf\nContent-Disposition: attachment\n\nxx\n--B--\n";
        let p = parse(raw);
        assert_eq!(p.html.trim(), "<p>Café</p>");
        assert!(p.text.is_empty());
        assert_eq!(p.attachments.len(), 1);
    }

    #[test]
    fn a_named_base64_part_is_an_attachment() {
        let raw = b"To: reply+k@x.example\nContent-Type: multipart/mixed; boundary=B\n\n--B\nContent-Type: text/plain\n\nhi\n--B\nContent-Type: image/png; name=\"a.png\"\nContent-Transfer-Encoding: base64\n\naGVsbG8=\n--B--\n";
        let p = parse(raw);
        assert_eq!(p.text.trim(), "hi");
        assert_eq!(p.attachments, [Attachment { filename: "a.png".into(), bytes: b"hello".to_vec() }]);
    }
}
