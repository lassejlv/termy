//! Encryption at rest for provider credentials.
//!
//! Provider OAuth tokens are sealed with XChaCha20-Poly1305 under a dedicated
//! key (`TERMY_API_ENCRYPTION_KEY`), separate from the auth signing secret so
//! rotating one cannot invalidate the other. Stored format is
//! `v1:<base64(nonce || ciphertext)>`; the version prefix leaves room for key
//! rotation.

use anyhow::Context as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, OsRng};
use chacha20poly1305::{AeadCore, Key, KeyInit as _, XChaCha20Poly1305, XNonce};

const VERSION_PREFIX: &str = "v1:";
const NONCE_LEN: usize = 24;

#[derive(Clone)]
pub(crate) struct TokenCipher {
    cipher: XChaCha20Poly1305,
}

impl TokenCipher {
    /// Builds a cipher from a base64-encoded 32-byte key.
    pub(crate) fn from_base64_key(encoded: &str) -> anyhow::Result<Self> {
        let key_bytes = BASE64
            .decode(encoded.trim())
            .context("TERMY_API_ENCRYPTION_KEY must be valid base64")?;
        anyhow::ensure!(
            key_bytes.len() == 32,
            "TERMY_API_ENCRYPTION_KEY must decode to exactly 32 bytes, got {}",
            key_bytes.len()
        );
        Ok(Self {
            cipher: XChaCha20Poly1305::new(Key::from_slice(&key_bytes)),
        })
    }

    pub(crate) fn encrypt(&self, plaintext: &str) -> String {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .expect("XChaCha20-Poly1305 encryption is infallible for in-memory buffers");
        let mut sealed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&ciphertext);
        format!("{VERSION_PREFIX}{}", BASE64.encode(sealed))
    }

    pub(crate) fn decrypt(&self, sealed: &str) -> anyhow::Result<String> {
        let encoded = sealed
            .strip_prefix(VERSION_PREFIX)
            .context("unsupported sealed token version")?;
        let bytes = BASE64
            .decode(encoded)
            .context("sealed token is not valid base64")?;
        anyhow::ensure!(bytes.len() > NONCE_LEN, "sealed token is too short");
        let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
        let plaintext = self
            .cipher
            .decrypt(XNonce::from_slice(nonce), ciphertext)
            .map_err(|_| anyhow::anyhow!("sealed token failed authentication"))?;
        String::from_utf8(plaintext).context("decrypted token is not valid UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::TokenCipher;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;

    fn cipher() -> TokenCipher {
        TokenCipher::from_base64_key(&BASE64.encode([7u8; 32])).expect("build cipher")
    }

    #[test]
    fn roundtrip_restores_plaintext() {
        let cipher = cipher();
        let sealed = cipher.encrypt("railway-access-token");
        assert!(sealed.starts_with("v1:"));
        assert_eq!(cipher.decrypt(&sealed).unwrap(), "railway-access-token");
    }

    #[test]
    fn encryption_is_randomized_per_call() {
        let cipher = cipher();
        assert_ne!(cipher.encrypt("same"), cipher.encrypt("same"));
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let cipher = cipher();
        let sealed = cipher.encrypt("secret");
        let mut bytes = BASE64.decode(sealed.strip_prefix("v1:").unwrap()).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let tampered = format!("v1:{}", BASE64.encode(bytes));
        assert!(cipher.decrypt(&tampered).is_err());
    }

    #[test]
    fn unknown_version_is_rejected() {
        let cipher = cipher();
        assert!(cipher.decrypt("v2:AAAA").is_err());
        assert!(cipher.decrypt("plain-token").is_err());
    }

    #[test]
    fn wrong_key_fails_authentication() {
        let sealed = cipher().encrypt("secret");
        let other = TokenCipher::from_base64_key(&BASE64.encode([8u8; 32])).unwrap();
        assert!(other.decrypt(&sealed).is_err());
    }

    #[test]
    fn key_length_is_validated() {
        assert!(TokenCipher::from_base64_key(&BASE64.encode([1u8; 16])).is_err());
        assert!(TokenCipher::from_base64_key("not-base64!!").is_err());
    }
}
