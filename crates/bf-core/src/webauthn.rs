//! Passkeys (WebAuthn level 2,), the subset the forum needs: ES256 (P-256)
//! credentials with "none" attestation, discoverable or not. Pure Rust, no OpenSSL.
//!
//! Registration checks the client data (type, challenge, origin), the RP ID hash and
//! the user-presence flag, then extracts the credential id and COSE public key.
//! Authentication checks the same, verifies the ECDSA signature over
//! `authenticatorData || SHA-256(clientDataJSON)` and rejects a sign count that went
//! backwards (a cloned authenticator).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ciborium::value::Value;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_AT: u8 = 0x40;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("malformed {0}")]
    Malformed(&'static str),
    #[error("wrong ceremony type")]
    Type,
    #[error("challenge mismatch")]
    Challenge,
    #[error("origin mismatch")]
    Origin,
    #[error("relying party mismatch")]
    RpId,
    #[error("user was not present")]
    Presence,
    #[error("only ES256 passkeys are supported")]
    Algorithm,
    #[error("bad signature")]
    Signature,
    #[error("sign count went backwards; the authenticator may be cloned")]
    Counter,
}

impl From<Error> for crate::AppError {
    fn from(e: Error) -> Self {
        crate::AppError::BadRequest(e.to_string())
    }
}

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(s: &str) -> Result<Vec<u8>, Error> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|_| Error::Malformed("base64"))
}

/// Who we are to authenticators.
pub struct RelyingParty<'a> {
    /// The site's host name, e.g. `forum.example.com`.
    pub id: &'a str,
    /// The exact origin pages are served from, e.g. `https://forum.example.com`.
    pub origin: &'a str,
}

impl RelyingParty<'_> {
    /// From the public base URL: the origin is the URL itself, the id its host.
    pub fn from_base_url(base: &str) -> Option<RelyingParty<'_>> {
        let rest = base.split_once("://")?.1;
        let host = rest.split(['/', ':']).next().filter(|h| !h.is_empty())?;
        Some(RelyingParty { id: host, origin: base.trim_end_matches('/') })
    }
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
}

fn check_client_data(json: &[u8], kind: &str, challenge: &[u8], rp: &RelyingParty) -> Result<(), Error> {
    let cd: ClientData = serde_json::from_slice(json).map_err(|_| Error::Malformed("clientDataJSON"))?;
    if cd.kind != kind {
        return Err(Error::Type);
    }
    // Compare decoded bytes so padding or encoding differences cannot slip through.
    if unb64(&cd.challenge)? != challenge {
        return Err(Error::Challenge);
    }
    if cd.origin != rp.origin {
        return Err(Error::Origin);
    }
    Ok(())
}

struct AuthData<'a> {
    flags: u8,
    sign_count: u32,
    rest: &'a [u8],
}

fn parse_auth_data<'a>(data: &'a [u8], rp: &RelyingParty) -> Result<AuthData<'a>, Error> {
    if data.len() < 37 {
        return Err(Error::Malformed("authenticatorData"));
    }
    if data[..32] != Sha256::digest(rp.id.as_bytes())[..] {
        return Err(Error::RpId);
    }
    let flags = data[32];
    if flags & FLAG_UP == 0 {
        return Err(Error::Presence);
    }
    let sign_count = u32::from_be_bytes([data[33], data[34], data[35], data[36]]);
    Ok(AuthData { flags, sign_count, rest: &data[37..] })
}

/// A stored passkey.
#[derive(Debug, Clone, PartialEq)]
pub struct Credential {
    pub id: Vec<u8>,
    /// Uncompressed SEC1 point (65 bytes) of the P-256 public key.
    pub public_key: Vec<u8>,
    pub sign_count: u32,
    /// The authenticator verified the user (PIN, biometrics).
    pub user_verified: bool,
}

fn cose_int(map: &[(Value, Value)], key: i64) -> Option<&Value> {
    map.iter().find(|(k, _)| k.as_integer().map(i128::from) == Some(i128::from(key))).map(|(_, v)| v)
}

/// COSE_Key (EC2, ES256, P-256) → uncompressed SEC1 point.
fn cose_to_sec1(cose: &Value) -> Result<Vec<u8>, Error> {
    let map = cose.as_map().ok_or(Error::Malformed("COSE key"))?;
    let int = |k| cose_int(map, k).and_then(ciborium::Value::as_integer).map(i128::from);
    // kty 2 = EC2, alg -7 = ES256, crv 1 = P-256.
    if int(1) != Some(2) || int(3) != Some(-7) || int(-1) != Some(1) {
        return Err(Error::Algorithm);
    }
    let bytes = |k| cose_int(map, k).and_then(|v| v.as_bytes()).filter(|b| b.len() == 32);
    let (x, y) = (bytes(-2).ok_or(Error::Malformed("COSE x"))?, bytes(-3).ok_or(Error::Malformed("COSE y"))?);
    // Uncompressed SEC1 form.
    let mut point = Vec::with_capacity(65);
    point.push(4);
    point.extend_from_slice(x);
    point.extend_from_slice(y);
    // Reject points that are not on the curve now rather than at login.
    VerifyingKey::from_sec1_bytes(&point).map_err(|_| Error::Malformed("public key"))?;
    Ok(point)
}

/// Verifies a registration (`navigator.credentials.create`) response.
pub fn verify_registration(
    rp: &RelyingParty,
    challenge: &[u8],
    client_data_json: &[u8],
    attestation_object: &[u8],
) -> Result<Credential, Error> {
    check_client_data(client_data_json, "webauthn.create", challenge, rp)?;
    let att: Value =
        ciborium::de::from_reader(attestation_object).map_err(|_| Error::Malformed("attestationObject"))?;
    let att = att.as_map().ok_or(Error::Malformed("attestationObject"))?;
    let auth_data = att
        .iter()
        .find(|(k, _)| k.as_text() == Some("authData"))
        .and_then(|(_, v)| v.as_bytes())
        .ok_or(Error::Malformed("authData"))?;
    // Attestation is not requested ("none"); whatever statement came is not trusted or needed.
    let ad = parse_auth_data(auth_data, rp)?;
    if ad.flags & FLAG_AT == 0 || ad.rest.len() < 18 {
        return Err(Error::Malformed("attested credential data"));
    }
    let id_len = u16::from_be_bytes([ad.rest[16], ad.rest[17]]) as usize;
    let id = ad.rest.get(18..18 + id_len).ok_or(Error::Malformed("credential id"))?.to_vec();
    if id.is_empty() || id.len() > 1023 {
        return Err(Error::Malformed("credential id"));
    }
    let cose: Value = ciborium::de::from_reader(&ad.rest[18 + id_len..]).map_err(|_| Error::Malformed("COSE key"))?;
    Ok(Credential {
        id,
        public_key: cose_to_sec1(&cose)?,
        sign_count: ad.sign_count,
        user_verified: ad.flags & FLAG_UV != 0,
    })
}

/// Verifies an authentication (`navigator.credentials.get`) response against the stored
/// credential. Returns the new sign count to store.
pub fn verify_authentication(
    rp: &RelyingParty,
    challenge: &[u8],
    stored: &Credential,
    client_data_json: &[u8],
    authenticator_data: &[u8],
    signature: &[u8],
) -> Result<u32, Error> {
    check_client_data(client_data_json, "webauthn.get", challenge, rp)?;
    let ad = parse_auth_data(authenticator_data, rp)?;
    // A passkey stands in for password + TOTP, so possession alone is not enough.
    if stored.user_verified && ad.flags & FLAG_UV == 0 {
        return Err(Error::Malformed("user verification"));
    }
    let key = VerifyingKey::from_sec1_bytes(&stored.public_key).map_err(|_| Error::Malformed("stored key"))?;
    let sig = Signature::from_der(signature).map_err(|_| Error::Signature)?;
    let mut signed = authenticator_data.to_vec();
    signed.extend_from_slice(&Sha256::digest(client_data_json));
    key.verify(&signed, &sig).map_err(|_| Error::Signature)?;
    // Authenticators that do not count report 0 every time; that is allowed.
    if (ad.sign_count != 0 || stored.sign_count != 0) && ad.sign_count <= stored.sign_count {
        return Err(Error::Counter);
    }
    Ok(ad.sign_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::SigningKey;
    use p256::ecdsa::signature::Signer;

    const RP: RelyingParty = RelyingParty { id: "forum.test", origin: "https://forum.test" };

    /// A software authenticator: enough to drive both ceremonies.
    struct Soft {
        key: SigningKey,
        id: Vec<u8>,
        count: u32,
    }

    impl Soft {
        fn new() -> Self {
            Soft { key: SigningKey::from_slice(&[7u8; 32]).unwrap(), id: b"credential-1".to_vec(), count: 0 }
        }

        fn client_data(kind: &str, challenge: &[u8], origin: &str) -> Vec<u8> {
            format!(r#"{{"type":"{kind}","challenge":"{}","origin":"{origin}","crossOrigin":false}}"#, b64(challenge))
                .into_bytes()
        }

        fn auth_data(&self, rp_id: &str, flags: u8, extra: &[u8]) -> Vec<u8> {
            let mut d = Sha256::digest(rp_id.as_bytes()).to_vec();
            d.push(flags);
            d.extend_from_slice(&self.count.to_be_bytes());
            d.extend_from_slice(extra);
            d
        }

        fn register(&self, challenge: &[u8], rp_id: &str, origin: &str) -> (Vec<u8>, Vec<u8>) {
            let point = self.key.verifying_key().to_sec1_point(false);
            let cose = Value::Map(vec![
                (Value::Integer(1.into()), Value::Integer(2.into())),
                (Value::Integer(3.into()), Value::Integer((-7).into())),
                (Value::Integer((-1).into()), Value::Integer(1.into())),
                (Value::Integer((-2).into()), Value::Bytes(point.x().unwrap().to_vec())),
                (Value::Integer((-3).into()), Value::Bytes(point.y().unwrap().to_vec())),
            ]);
            let mut acd = vec![0u8; 16];
            acd.extend_from_slice(&(self.id.len() as u16).to_be_bytes());
            acd.extend_from_slice(&self.id);
            ciborium::ser::into_writer(&cose, &mut acd).unwrap();
            let auth = self.auth_data(rp_id, FLAG_UP | FLAG_UV | FLAG_AT, &acd);
            let att = Value::Map(vec![
                (Value::Text("fmt".into()), Value::Text("none".into())),
                (Value::Text("attStmt".into()), Value::Map(vec![])),
                (Value::Text("authData".into()), Value::Bytes(auth)),
            ]);
            let mut out = Vec::new();
            ciborium::ser::into_writer(&att, &mut out).unwrap();
            (Self::client_data("webauthn.create", challenge, origin), out)
        }

        fn sign_in(&mut self, challenge: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
            self.count += 1;
            let cd = Self::client_data("webauthn.get", challenge, RP.origin);
            let ad = self.auth_data(RP.id, FLAG_UP | FLAG_UV, &[]);
            let mut msg = ad.clone();
            msg.extend_from_slice(&Sha256::digest(&cd));
            let sig: Signature = self.key.sign(&msg);
            (cd, ad, sig.to_der().as_bytes().to_vec())
        }
    }

    #[test]
    fn register_then_sign_in() {
        let mut soft = Soft::new();
        let (cd, att) = soft.register(b"challenge-1", RP.id, RP.origin);
        let cred = verify_registration(&RP, b"challenge-1", &cd, &att).unwrap();
        assert_eq!(cred.id, soft.id);
        assert!(cred.user_verified);
        let (cd, ad, sig) = soft.sign_in(b"challenge-2");
        assert_eq!(verify_authentication(&RP, b"challenge-2", &cred, &cd, &ad, &sig), Ok(1));
    }

    #[test]
    fn registration_rejects_wrong_challenge_origin_and_rp() {
        let soft = Soft::new();
        let (cd, att) = soft.register(b"c", RP.id, RP.origin);
        assert_eq!(verify_registration(&RP, b"other", &cd, &att), Err(Error::Challenge));
        let (cd, att) = soft.register(b"c", RP.id, "https://evil.test");
        assert_eq!(verify_registration(&RP, b"c", &cd, &att), Err(Error::Origin));
        let (cd, att) = soft.register(b"c", "evil.test", RP.origin);
        assert_eq!(verify_registration(&RP, b"c", &cd, &att), Err(Error::RpId));
    }

    #[test]
    fn sign_in_rejects_tampering_replays_and_clones() {
        let mut soft = Soft::new();
        let (cd, att) = soft.register(b"r", RP.id, RP.origin);
        let mut cred = verify_registration(&RP, b"r", &cd, &att).unwrap();
        let (cd, ad, sig) = soft.sign_in(b"a");
        assert_eq!(verify_authentication(&RP, b"wrong", &cred, &cd, &ad, &sig), Err(Error::Challenge));
        let mut bad = sig.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert_eq!(verify_authentication(&RP, b"a", &cred, &cd, &ad, &bad), Err(Error::Signature));
        cred.sign_count = verify_authentication(&RP, b"a", &cred, &cd, &ad, &sig).unwrap();
        // The same response again: the counter did not move, so it is a replay or a clone.
        assert_eq!(verify_authentication(&RP, b"a", &cred, &cd, &ad, &sig), Err(Error::Counter));
        let mut other = Soft::new();
        other.key = SigningKey::from_slice(&[9u8; 32]).unwrap();
        other.count = 5;
        let (cd, ad, sig) = other.sign_in(b"b");
        assert_eq!(verify_authentication(&RP, b"b", &cred, &cd, &ad, &sig), Err(Error::Signature));
    }

    #[test]
    fn relying_party_from_base_url() {
        let rp = RelyingParty::from_base_url("https://forum.example.com/").unwrap();
        assert_eq!((rp.id, rp.origin), ("forum.example.com", "https://forum.example.com"));
        assert_eq!(RelyingParty::from_base_url("http://127.0.0.1:8099").unwrap().id, "127.0.0.1");
    }
}
