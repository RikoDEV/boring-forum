//! OAuth 2 / OIDC login (Google, GitHub, Discord, generic OIDC): provider
//! endpoints, PKCE (RFC 7636) and the authorize URL. The token exchange is an
//! HTTP call made by the server; this module holds the parts that must be exact.

use rand::{TryRng, rngs::SysRng};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub key: &'static str,
    pub authorize_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub scope: &'static str,
    /// Apple needs `response_mode=form_post` when asking for the name and email.
    pub form_post: bool,
}

impl Provider {
    pub fn builtin(key: &str) -> Option<Provider> {
        let p = |key, a: &str, t: &str, u: &str, scope| Provider {
            key,
            authorize_url: a.into(),
            token_url: t.into(),
            userinfo_url: u.into(),
            scope,
            form_post: false,
        };
        Some(match key {
            "google" => p(
                "google",
                "https://accounts.google.com/o/oauth2/v2/auth",
                "https://oauth2.googleapis.com/token",
                "https://openidconnect.googleapis.com/v1/userinfo",
                "openid email profile",
            ),
            "github" => p(
                "github",
                "https://github.com/login/oauth/authorize",
                "https://github.com/login/oauth/access_token",
                "https://api.github.com/user",
                "read:user user:email",
            ),
            "discord" => p(
                "discord",
                "https://discord.com/oauth2/authorize",
                "https://discord.com/api/oauth2/token",
                "https://discord.com/api/users/@me",
                "identify email",
            ),
            "microsoft" => p(
                "microsoft",
                "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
                "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                "https://graph.microsoft.com/oidc/userinfo",
                "openid email profile",
            ),
            _ => return None,
        })
    }

    /// A generic OIDC provider from its discovery document's endpoints.
    pub fn oidc(authorize_url: &str, token_url: &str, userinfo_url: &str) -> Result<Provider, &'static str> {
        // Plain http is accepted only on the loopback interface (development, tests).
        let ok = |u: &str| {
            u.starts_with("https://") || u.starts_with("http://127.0.0.1:") || u.starts_with("http://localhost:")
        };
        if ![authorize_url, token_url, userinfo_url].into_iter().all(ok) {
            return Err("OIDC endpoints must use https");
        }
        Ok(Provider {
            key: "oidc",
            authorize_url: authorize_url.into(),
            token_url: token_url.into(),
            userinfo_url: userinfo_url.into(),
            scope: "openid email profile",
            form_post: false,
        })
    }

    /// Sign in with Apple. `base` is `https://appleid.apple.com` outside tests. Apple has no
    /// userinfo endpoint: the profile comes from the `id_token` in the token response.
    pub fn apple(base: &str) -> Result<Provider, &'static str> {
        let mut p =
            Provider::oidc(&format!("{base}/auth/authorize"), &format!("{base}/auth/token"), &format!("{base}/"))?;
        p.key = "apple";
        p.scope = "name email";
        p.form_post = true;
        Ok(p)
    }
}

fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// Per-login secrets kept server-side (or in a signed, short-lived cookie).
#[derive(Debug, Clone)]
pub struct LoginStart {
    pub state: String,
    pub verifier: String,
    pub url: String,
}

pub fn pkce_challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

pub fn start(p: &Provider, client_id: &str, redirect_uri: &str) -> LoginStart {
    let mut b = [0u8; 32];
    #[expect(
        clippy::expect_used,
        reason = "the operating system's random source is always present; without it nothing secure can run"
    )]
    SysRng.try_fill_bytes(&mut b).expect("the system random source is available");
    let verifier = b64url(&b);
    let state = crate::auth::random_token();
    let mode = if p.form_post { "&response_mode=form_post" } else { "" };
    let url = format!(
        "{}?response_type=code{mode}&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        p.authorize_url,
        crate::html::url_encode(client_id),
        crate::html::url_encode(redirect_uri),
        crate::html::url_encode(p.scope),
        state,
        pkce_challenge(&verifier),
    );
    LoginStart { state, verifier, url }
}

/// Apple's client secret: an ES256 JWT signed with the `.p8` key from the developer account.
pub fn apple_client_secret(
    team_id: &str,
    key_id: &str,
    client_id: &str,
    pem: &str,
    audience: &str,
    now: i64,
) -> Result<String, String> {
    use p256::ecdsa::{Signature, SigningKey, signature::Signer};
    use p256::pkcs8::DecodePrivateKey;
    let key = SigningKey::from_pkcs8_pem(pem).map_err(|e| format!("Apple key: {e}"))?;
    let header = serde_json::json!({ "alg": "ES256", "kid": key_id }).to_string();
    let claims = serde_json::json!({
        "iss": team_id, "iat": now, "exp": now + 300, "aud": audience, "sub": client_id,
    })
    .to_string();
    let body = format!("{}.{}", b64url(header.as_bytes()), b64url(claims.as_bytes()));
    let sig: Signature = key.sign(body.as_bytes());
    Ok(format!("{body}.{}", b64url(&sig.to_bytes())))
}

/// Claims of an `id_token` received directly from the provider's token endpoint over TLS,
/// where OIDC Core 3.1.3.7 lets the signature check be skipped. Issuer, audience and expiry
/// are still enforced.
pub fn id_token_claims(token: &str, issuer: &str, client_id: &str, now: i64) -> Result<serde_json::Value, String> {
    use base64::Engine;
    let payload = token.split('.').nth(1).ok_or("malformed id_token")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).map_err(|_| "malformed id_token")?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "malformed id_token")?;
    if claims["iss"] != issuer {
        return Err("id_token has the wrong issuer".into());
    }
    if claims["aud"] != client_id {
        return Err("id_token has the wrong audience".into());
    }
    if claims["exp"].as_i64().is_none_or(|exp| exp < now) {
        return Err("id_token has expired".into());
    }
    Ok(claims)
}

/// Body for the token request (`application/x-www-form-urlencoded`).
pub fn token_request_body(
    code: &str,
    verifier: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> String {
    format!(
        "grant_type=authorization_code&code={}&code_verifier={}&client_id={}&client_secret={}&redirect_uri={}",
        crate::html::url_encode(code),
        crate::html::url_encode(verifier),
        crate::html::url_encode(client_id),
        crate::html::url_encode(client_secret),
        crate::html::url_encode(redirect_uri),
    )
}

/// The callback's `state` must equal the one we issued (CSRF on the login flow).
pub fn state_matches(issued: &str, returned: &str) -> bool {
    crate::auth::csrf_matches(Some(issued), Some(returned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_rfc7636_vector() {
        // RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn base64url_unpadded() {
        assert_eq!(b64url(b"Man"), "TWFu");
        assert_eq!(b64url(b"Ma"), "TWE");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn authorize_url_has_pkce_and_state() {
        let p = Provider::builtin("github").unwrap();
        let s = start(&p, "abc", "https://f.test/auth/github/callback");
        assert!(s.url.starts_with("https://github.com/login/oauth/authorize?response_type=code&client_id=abc"));
        assert!(s.url.contains("redirect_uri=https%3A%2F%2Ff.test%2Fauth%2Fgithub%2Fcallback"));
        assert!(s.url.contains(&format!("state={}", s.state)));
        assert!(s.url.contains(&format!("code_challenge={}", pkce_challenge(&s.verifier))));
        assert!((43..=128).contains(&s.verifier.len()));
        assert!(state_matches(&s.state, &s.state));
        assert!(!state_matches(&s.state, "forged"));
    }

    #[test]
    fn apple_secret_is_a_valid_es256_jwt() {
        use base64::Engine;
        use p256::ecdsa::{Signature, SigningKey, signature::Verifier};
        use p256::pkcs8::{EncodePrivateKey, LineEnding};
        let key = <SigningKey as p256::elliptic_curve::Generate>::generate();
        let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let jwt = apple_client_secret("TEAM", "KID", "com.example", &pem, "https://appleid.apple.com", 1000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        let d = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&d(parts[1])).unwrap();
        assert_eq!(
            (claims["iss"].as_str(), claims["sub"].as_str(), claims["exp"].as_i64()),
            (Some("TEAM"), Some("com.example"), Some(1300))
        );
        let sig = Signature::from_slice(&d(parts[2])).unwrap();
        key.verifying_key().verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).unwrap();
        assert!(apple_client_secret("T", "K", "C", "not a key", "a", 0).is_err());
    }

    #[test]
    fn id_token_checks() {
        let tok = |c: serde_json::Value| format!("x.{}.y", b64url(c.to_string().as_bytes()));
        let good = serde_json::json!({"iss":"i","aud":"a","exp":100,"sub":"s"});
        assert_eq!(id_token_claims(&tok(good.clone()), "i", "a", 50).unwrap()["sub"], "s");
        assert!(id_token_claims(&tok(good.clone()), "other", "a", 50).is_err());
        assert!(id_token_claims(&tok(good.clone()), "i", "other", 50).is_err());
        assert!(id_token_claims(&tok(good), "i", "a", 200).is_err());
        assert!(id_token_claims("garbage", "i", "a", 0).is_err());
    }

    #[test]
    fn apple_uses_form_post() {
        let p = Provider::apple("https://appleid.apple.com").unwrap();
        assert!(start(&p, "c", "https://x/cb").url.contains("response_mode=form_post"));
        assert!(!start(&Provider::builtin("google").unwrap(), "c", "https://x/cb").url.contains("form_post"));
    }

    #[test]
    fn providers() {
        for k in ["google", "github", "discord", "microsoft"] {
            assert_eq!(Provider::builtin(k).unwrap().key, k);
        }
        assert!(Provider::builtin("myspace").is_none());
        assert!(Provider::oidc("http://x", "https://y", "https://z").is_err());
        assert!(Provider::oidc("http://localhost.evil.com/a", "https://y", "https://z").is_err());
        assert!(
            Provider::oidc("http://127.0.0.1:9000/a", "http://127.0.0.1:9000/t", "http://127.0.0.1:9000/u").is_ok()
        );
    }
}
