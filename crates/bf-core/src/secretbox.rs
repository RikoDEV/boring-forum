//! Encryption at rest for sensitive columns: TOTP secrets and webhook signing keys.
//!
//! The key never lives in the database. It comes from `BF_SECRET_KEY`, a base64url-encoded 32
//! bytes (`boring-forum secret-key` prints one). Retired keys stay readable by listing them in
//! `BF_SECRET_KEY_OLD`, comma-separated, so a rotation is: generate a key, move the old value to
//! `BF_SECRET_KEY_OLD`, restart, then run `boring-forum rotate-secrets` to rewrite every row with
//! the new key and drop the old value from the environment.
//!
//! Stored form is `bf1.<key id>.<nonce>.<ciphertext>`, all base64url. The key id is the first 8
//! hex characters of SHA-256 over the key bytes, so a value names the key that sealed it without
//! revealing it, and a rotation can tell rewritten rows from ones still on the old key.
//!
//! A value that does not start with `bf1.` is returned unchanged by [`Keyring::open`]. That keeps
//! a database written before the key existed readable; those rows are sealed the next time they
//! are written, or all at once by `rotate-secrets`.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use rand::{TryRng, rngs::SysRng};
use sha2::{Digest, Sha256};

const PREFIX: &str = "bf1";

/// One AES-256-GCM key and the id that names it in stored values.
struct Key {
    id: String,
    cipher: Aes256Gcm,
}

impl Key {
    fn new(bytes: &[u8; 32]) -> Self {
        let digest = Sha256::digest(bytes);
        let id = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        // `new_from_slice` only fails on the wrong length, and the type guarantees 32 bytes.
        #[expect(clippy::expect_used, reason = "the argument is a [u8; 32], the one length AES-256 takes")]
        let cipher = Aes256Gcm::new_from_slice(bytes).expect("32-byte key");
        Self { id, cipher }
    }
}

/// The current key plus any retired keys that stored values may still name.
pub struct Keyring {
    keys: Vec<Key>,
}

/// Parses one base64url key of exactly 32 bytes.
fn parse_key(raw: &str) -> Result<[u8; 32], String> {
    let bytes = B64
        .decode(raw.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(raw.trim()))
        .map_err(|_| "not base64".to_owned())?;
    let len = bytes.len();
    bytes.try_into().map_err(|_| format!("{len} bytes, expected 32"))
}

impl Keyring {
    /// Builds a keyring from `BF_SECRET_KEY` and `BF_SECRET_KEY_OLD`. `Ok(None)` means no key is
    /// configured, which leaves the sensitive columns in plain text; an invalid key is an error,
    /// so a typo cannot silently turn encryption off.
    pub fn from_env() -> Result<Option<Self>, String> {
        let Some(current) = std::env::var("BF_SECRET_KEY").ok().filter(|v| !v.trim().is_empty()) else {
            return Ok(None);
        };
        let mut keys = vec![Key::new(&parse_key(&current).map_err(|e| format!("BF_SECRET_KEY: {e}"))?)];
        for (i, old) in std::env::var("BF_SECRET_KEY_OLD").unwrap_or_default().split(',').enumerate() {
            if old.trim().is_empty() {
                continue;
            }
            let bytes = parse_key(old).map_err(|e| format!("BF_SECRET_KEY_OLD[{i}]: {e}"))?;
            keys.push(Key::new(&bytes));
        }
        Ok(Some(Self { keys }))
    }

    #[cfg(test)]
    fn from_keys(keys: &[[u8; 32]]) -> Self {
        Self { keys: keys.iter().map(Key::new).collect() }
    }

    /// The id of the key new values are sealed with.
    pub fn current_id(&self) -> &str {
        &self.keys[0].id
    }

    /// Seals a value with the current key. A fresh 96-bit nonce per call, as AES-GCM requires.
    pub fn seal(&self, plaintext: &str) -> String {
        let key = &self.keys[0];
        let mut nonce = [0u8; 12];
        #[expect(
            clippy::expect_used,
            reason = "the operating system's random source is always present; without it nothing secure can run"
        )]
        SysRng.try_fill_bytes(&mut nonce).expect("the system random source is available");
        // AES-GCM only fails here on a length that cannot occur for our inputs.
        #[expect(
            clippy::expect_used,
            reason = "AES-GCM fails only on a plaintext beyond 2^36 bytes, which a stored secret never is"
        )]
        let sealed = key.cipher.encrypt((&nonce).into(), plaintext.as_bytes()).expect("aes-gcm seal");
        format!("{PREFIX}.{}.{}.{}", key.id, B64.encode(nonce), B64.encode(sealed))
    }

    /// Opens a stored value. A value that is not in the sealed format is returned as it is, so
    /// rows written before a key was configured keep working. `None` means the value is sealed
    /// but unreadable: the key that sealed it is not in the keyring, or the data was tampered
    /// with, and the caller must treat it as missing rather than guess.
    pub fn open(&self, stored: &str) -> Option<String> {
        let Some(parts) = sealed_parts(stored) else { return Some(stored.to_owned()) };
        let (id, nonce, ciphertext) = parts;
        let key = self.keys.iter().find(|k| k.id == id)?;
        let nonce: [u8; 12] = B64.decode(nonce).ok()?.try_into().ok()?;
        let sealed = B64.decode(ciphertext).ok()?;
        let plaintext = key.cipher.decrypt((&nonce).into(), sealed.as_slice()).ok()?;
        String::from_utf8(plaintext).ok()
    }

    /// True when a stored value is not sealed with the current key, so `rotate-secrets` should
    /// rewrite it. That covers both plain text and a value under a retired key.
    pub fn stale(&self, stored: &str) -> bool {
        sealed_parts(stored).is_none_or(|(id, _, _)| id != self.keys[0].id)
    }
}

/// Splits `bf1.<id>.<nonce>.<ciphertext>`, or `None` when the value is not in that form.
fn sealed_parts(stored: &str) -> Option<(&str, &str, &str)> {
    let rest = stored.strip_prefix(PREFIX)?.strip_prefix('.')?;
    let (id, rest) = rest.split_once('.')?;
    let (nonce, ciphertext) = rest.split_once('.')?;
    (!id.is_empty() && !nonce.is_empty() && !ciphertext.is_empty()).then_some((id, nonce, ciphertext))
}

/// Prints a new key for `BF_SECRET_KEY` (`boring-forum secret-key`).
pub fn generate_key() -> String {
    let mut bytes = [0u8; 32];
    #[expect(
        clippy::expect_used,
        reason = "the operating system's random source is always present; without it nothing secure can run"
    )]
    SysRng.try_fill_bytes(&mut bytes).expect("the system random source is available");
    format!("BF_SECRET_KEY={}\n", B64.encode(bytes))
}

/// The process-wide keyring, read once. Built here rather than carried in `AppState` so the
/// database layer can seal a column without every caller passing a key down.
pub fn keyring() -> Option<&'static Keyring> {
    static RING: std::sync::LazyLock<Option<Keyring>> = std::sync::LazyLock::new(|| match Keyring::from_env() {
        Ok(ring) => {
            if ring.is_none() {
                tracing::warn!("BF_SECRET_KEY is not set: TOTP and webhook secrets are stored in plain text");
            }
            ring
        }
        // A bad key must not fall back to plain text silently.
        #[expect(
            clippy::panic,
            reason = "startup: a mistyped BF_SECRET_KEY must stop the server before it stores anything in plain text"
        )]
        Err(e) => panic!("{e}"),
    });
    RING.as_ref()
}

/// Seals a value for storage, or returns it unchanged when no key is configured.
pub fn seal(plaintext: &str) -> String {
    keyring().map_or_else(|| plaintext.to_owned(), |k| k.seal(plaintext))
}

/// Opens a stored value. Without a key, a sealed value cannot be read and comes back `None`.
pub fn open(stored: &str) -> Option<String> {
    match keyring() {
        Some(ring) => ring.open(stored),
        None if sealed_parts(stored).is_some() => None,
        None => Some(stored.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: [u8; 32] = [7u8; 32];
    const B: [u8; 32] = [9u8; 32];

    #[test]
    fn seals_and_opens() {
        let ring = Keyring::from_keys(&[A]);
        let sealed = ring.seal("JBSWY3DPEHPK3PXP");
        assert!(sealed.starts_with("bf1."));
        assert!(!sealed.contains("JBSWY3DPEHPK3PXP"));
        assert_eq!(ring.open(&sealed).as_deref(), Some("JBSWY3DPEHPK3PXP"));
    }

    #[test]
    fn nonce_differs_per_call() {
        let ring = Keyring::from_keys(&[A]);
        assert_ne!(ring.seal("same"), ring.seal("same"));
    }

    #[test]
    fn plain_text_passes_through() {
        let ring = Keyring::from_keys(&[A]);
        assert_eq!(ring.open("JBSWY3DPEHPK3PXP").as_deref(), Some("JBSWY3DPEHPK3PXP"));
        assert!(ring.stale("JBSWY3DPEHPK3PXP"));
    }

    #[test]
    fn retired_key_still_opens_but_is_stale() {
        let old = Keyring::from_keys(&[B]);
        let sealed = old.seal("secret");
        let rotated = Keyring::from_keys(&[A, B]);
        assert_eq!(rotated.open(&sealed).as_deref(), Some("secret"));
        assert!(rotated.stale(&sealed));
        assert!(!rotated.stale(&rotated.seal("secret")));
    }

    #[test]
    fn unknown_key_and_tampering_refuse() {
        let sealed = Keyring::from_keys(&[B]).seal("secret");
        assert_eq!(Keyring::from_keys(&[A]).open(&sealed), None);

        let ring = Keyring::from_keys(&[A]);
        let sealed = ring.seal("secret");
        let mut bytes = sealed.into_bytes();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(ring.open(&String::from_utf8(bytes).unwrap()), None);
    }

    #[test]
    fn key_ids_name_the_key() {
        assert_eq!(Keyring::from_keys(&[A]).current_id(), Keyring::from_keys(&[A]).current_id());
        assert_ne!(Keyring::from_keys(&[A]).current_id(), Keyring::from_keys(&[B]).current_id());
    }

    #[test]
    fn rejects_a_key_of_the_wrong_size() {
        assert!(parse_key(&B64.encode([1u8; 16])).is_err());
        assert!(parse_key("not base64!!").is_err());
        assert!(parse_key(&B64.encode([1u8; 32])).is_ok());
    }
}

#[cfg(test)]
mod rotation_tests;
