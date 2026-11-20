//! HTTP Signatures (draft-cavage, RSA-SHA256): the actors' key pairs, signing outgoing requests and the pieces needed to verify incoming ones.

use crate::http::render::DbResult;
use crate::state::AppState;
use axum::http::{HeaderMap, Method, Uri};
use base64::Engine;
use bf_core::AppError;
use bf_db::federation;
use rsa::pkcs1v15::{Signature, SigningKey, VerifyingKey};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer, Verifier};
use rsa::{PrivateKey as RsaPrivateKey, PublicKey as RsaPublicKey};

use super::*;

/// The key pair of a local actor, made on first use. The private key is sealed at rest.
pub(super) async fn key_for(
    state: &AppState,
    owner_type: &str,
    owner_id: i64,
) -> Result<(String, RsaPrivateKey), AppError> {
    let pool = state.db()?;
    let (public, sealed) = if let Some(k) = federation::key(pool, owner_type, owner_id).await.db()? {
        k
    } else {
        let (public, private) = tokio::task::spawn_blocking(|| {
            let key = RsaPrivateKey::new(&mut rand::rngs::SysRng, 2048).map_err(|e| e.to_string())?;
            let public = RsaPublicKey::from(&key).to_public_key_pem(LineEnding::LF).map_err(|e| e.to_string())?;
            let private = key.to_pkcs8_pem(LineEnding::LF).map_err(|e| e.to_string())?.to_string();
            Ok::<_, String>((public, private))
        })
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::internal)?;
        federation::insert_key(pool, owner_type, owner_id, &public, &bf_core::secretbox::seal(&private)).await.db()?
    };
    let pem = bf_core::secretbox::open(&sealed).ok_or_else(|| AppError::internal("actor key cannot be opened"))?;
    let key = RsaPrivateKey::from_pkcs8_pem(&pem).map_err(AppError::internal)?;
    Ok((public, key))
}

pub(super) async fn public_key_pem(state: &AppState, owner_type: &str, owner_id: i64) -> Result<String, AppError> {
    Ok(key_for(state, owner_type, owner_id).await?.0)
}

pub(super) fn digest_header(body: &[u8]) -> String {
    format!("SHA-256={}", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(body)))
}

pub(super) fn http_date() -> String {
    chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// Headers for a signed request (draft-cavage-http-signatures-12, RSA-SHA256).
pub(super) fn sign(
    key: &RsaPrivateKey,
    key_id: &str,
    method: &str,
    url: &url::Url,
    body: Option<&[u8]>,
) -> Vec<(String, String)> {
    let host = match url.port() {
        Some(p) => format!("{}:{p}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_owned(),
    };
    let target = match url.query() {
        Some(q) => format!("{} {}?{q}", method.to_ascii_lowercase(), url.path()),
        None => format!("{} {}", method.to_ascii_lowercase(), url.path()),
    };
    let date = http_date();
    let mut lines = vec![format!("(request-target): {target}"), format!("host: {host}"), format!("date: {date}")];
    let mut names = vec!["(request-target)", "host", "date"];
    let mut headers = vec![("date".to_owned(), date)];
    if let Some(b) = body {
        let d = digest_header(b);
        lines.push(format!("digest: {d}"));
        lines.push(format!("content-type: {AP_TYPE}"));
        names.extend(["digest", "content-type"]);
        headers.push(("digest".to_owned(), d));
        headers.push(("content-type".to_owned(), AP_TYPE.to_owned()));
    }
    let signer = SigningKey::<Sha256>::new(key.clone());
    let sig = signer.sign(lines.join("\n").as_bytes()).to_bytes();
    headers.push((
        "signature".to_owned(),
        format!(
            r#"keyId="{key_id}",algorithm="rsa-sha256",headers="{}",signature="{}""#,
            names.join(" "),
            base64::engine::general_purpose::STANDARD.encode(sig)
        ),
    ));
    headers
}

pub(super) struct SigParams {
    pub(super) key_id: String,
    pub(super) headers: Vec<String>,
    pub(super) signature: Vec<u8>,
}

pub(super) fn parse_signature(h: &str) -> Option<SigParams> {
    let mut key_id = None;
    let mut headers = None;
    let mut signature = None;
    for part in h.split(',') {
        let (k, v) = part.trim().split_once('=')?;
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "keyId" => key_id = Some(v.to_owned()),
            "headers" => headers = Some(v.split_whitespace().map(str::to_ascii_lowercase).collect::<Vec<_>>()),
            "signature" => signature = base64::engine::general_purpose::STANDARD.decode(v).ok(),
            _ => {}
        }
    }
    Some(SigParams {
        key_id: key_id?,
        headers: headers.unwrap_or_else(|| vec!["date".to_owned()]),
        signature: signature?,
    })
}

pub(super) fn signing_string(p: &SigParams, method: &Method, uri: &Uri, headers: &HeaderMap) -> Option<String> {
    let mut lines = Vec::new();
    for name in &p.headers {
        if name == "(request-target)" {
            let target = uri.path_and_query().map_or(uri.path(), |pq| pq.as_str());
            lines.push(format!("(request-target): {} {target}", method.as_str().to_ascii_lowercase()));
        } else {
            let v = headers.get(name.as_str())?.to_str().ok()?;
            lines.push(format!("{name}: {}", v.trim()));
        }
    }
    Some(lines.join("\n"))
}

pub(super) fn verify_with(pem: &str, msg: &str, sig: &[u8]) -> bool {
    use rsa::pkcs1::DecodeRsaPublicKey;
    let key = RsaPublicKey::from_public_key_pem(pem.trim()).or_else(|_| RsaPublicKey::from_pkcs1_pem(pem.trim()));
    let (Ok(key), Ok(sig)) = (key, Signature::try_from(sig)) else { return false };
    VerifyingKey::<Sha256>::new(key).verify(msg.as_bytes(), &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_round_trip_and_reject_tampering() {
        let key = RsaPrivateKey::new(&mut rand::rngs::SysRng, 1024).unwrap();
        let pem = RsaPublicKey::from(&key).to_public_key_pem(LineEnding::LF).unwrap();
        let url = url::Url::parse("https://forum.example/ap/inbox").unwrap();
        let body = br#"{"type":"Follow"}"#;
        let signed = sign(&key, "https://remote.example/u/a#main-key", "POST", &url, Some(body));
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("forum.example"));
        for (k, v) in &signed {
            headers
                .insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        let p = parse_signature(headers.get("signature").unwrap().to_str().unwrap()).unwrap();
        assert_eq!(p.key_id, "https://remote.example/u/a#main-key");
        let uri: Uri = "/ap/inbox".parse().unwrap();
        let msg = signing_string(&p, &Method::POST, &uri, &headers).unwrap();
        assert!(verify_with(&pem, &msg, &p.signature));
        let other: Uri = "/ap/u/1/inbox".parse().unwrap();
        let msg2 = signing_string(&p, &Method::POST, &other, &headers).unwrap();
        assert!(!verify_with(&pem, &msg2, &p.signature), "another path");
        assert_eq!(headers.get("digest").unwrap().to_str().unwrap(), digest_header(body));
    }
}

#[cfg(test)]
mod inbound_tests;
