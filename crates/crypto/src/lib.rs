//! Versioned authenticated-encryption envelopes shared by local storage and sync.
pub mod dpapi;

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use node2socks_domain::{AppError, AppResult, ErrorCode};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

const ENVELOPE_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretKey([u8; 32]);

impl SecretKey {
    pub fn random() -> Self {
        let mut key = [0_u8; 32];
        rand::rng().fill_bytes(&mut key);
        Self(key)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn derive(password: &[u8], salt: &[u8]) -> AppResult<Self> {
        let mut key = [0_u8; 32];
        let params = Params::new(64 * 1024, 3, 1, Some(32)).map_err(crypto_error)?;
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(password, salt, &mut key)
            .map_err(crypto_error)?;
        Ok(Self(key))
    }
    pub(crate) fn expose_for_platform_protection(&self) -> [u8; 32] {
        self.0
    }

    pub fn wrap(&self, wrapping_key: &SecretKey, aad: &[u8]) -> AppResult<Vec<u8>> {
        encrypt(wrapping_key, &self.0, aad)
    }

    pub fn unwrap(wrapping_key: &SecretKey, envelope: &[u8], aad: &[u8]) -> AppResult<Self> {
        let bytes = decrypt(wrapping_key, envelope, aad)?;
        let key: [u8; 32] = bytes.try_into().map_err(|_| {
            AppError::new(ErrorCode::CryptoError, "wrapped key must contain 32 bytes")
        })?;
        Ok(Self::from_bytes(key))
    }
    pub fn fingerprint(&self) -> String {
        hex_digest(&Sha256::digest(self.0))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope {
    pub version: u8,
    pub nonce: String,
    pub ciphertext: String,
}

pub fn encrypt(key: &SecretKey, plaintext: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(&key.0).map_err(crypto_error)?;
    let mut nonce = [0_u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(
            (&nonce).into(),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| AppError::new(ErrorCode::CryptoError, "encryption failed"))?;
    serde_json::to_vec(&Envelope {
        version: ENVELOPE_VERSION,
        nonce: STANDARD_NO_PAD.encode(nonce),
        ciphertext: STANDARD_NO_PAD.encode(ciphertext),
    })
    .map_err(crypto_error)
}

pub fn decrypt(key: &SecretKey, envelope: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
    let envelope: Envelope = serde_json::from_slice(envelope).map_err(crypto_error)?;
    if envelope.version != ENVELOPE_VERSION {
        return Err(AppError::new(
            ErrorCode::CryptoError,
            "unsupported envelope version",
        ));
    }
    let nonce = STANDARD_NO_PAD
        .decode(envelope.nonce)
        .map_err(crypto_error)?;
    if nonce.len() != NONCE_LEN {
        return Err(AppError::new(
            ErrorCode::CryptoError,
            "invalid nonce length",
        ));
    }
    let ciphertext = STANDARD_NO_PAD
        .decode(envelope.ciphertext)
        .map_err(crypto_error)?;
    Aes256Gcm::new_from_slice(&key.0)
        .map_err(crypto_error)?
        .decrypt(
            nonce.as_slice().into(),
            Payload {
                msg: &ciphertext,
                aad,
            },
        )
        .map_err(|_| AppError::new(ErrorCode::CryptoError, "authentication failed"))
}

fn crypto_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::CryptoError, error.to_string())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_and_tamper_wrong_key_invalid_nonce_fail() {
        let key = SecretKey::random();
        let encrypted = encrypt(&key, b"secret", b"subscription:1").unwrap();
        assert_eq!(
            decrypt(&key, &encrypted, b"subscription:1").unwrap(),
            b"secret"
        );
        assert!(decrypt(&SecretKey::random(), &encrypted, b"subscription:1").is_err());

        let mut value: Envelope = serde_json::from_slice(&encrypted).unwrap();
        value.ciphertext.push('A');
        assert!(
            decrypt(
                &key,
                &serde_json::to_vec(&value).unwrap(),
                b"subscription:1"
            )
            .is_err()
        );
        value.nonce = STANDARD_NO_PAD.encode([1_u8; 3]);
        assert!(
            decrypt(
                &key,
                &serde_json::to_vec(&value).unwrap(),
                b"subscription:1"
            )
            .is_err()
        );
    }

    #[test]
    fn argon2id_derivation_is_stable_and_salt_scoped() {
        let a = SecretKey::derive(b"correct horse", b"0123456789abcdef").unwrap();
        let b = SecretKey::derive(b"correct horse", b"0123456789abcdef").unwrap();
        let c = SecretKey::derive(b"correct horse", b"fedcba9876543210").unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(a.fingerprint(), c.fingerprint());
    }

    #[test]
    fn key_wrap_roundtrip_rejects_wrong_password_key() {
        let vault = SecretKey::random();
        let wrapping = SecretKey::derive(b"password", b"0123456789abcdef").unwrap();
        let envelope = vault.wrap(&wrapping, b"vault-bootstrap-v1").unwrap();
        let restored = SecretKey::unwrap(&wrapping, &envelope, b"vault-bootstrap-v1").unwrap();
        assert_eq!(vault.fingerprint(), restored.fingerprint());
        assert!(SecretKey::unwrap(&SecretKey::random(), &envelope, b"vault-bootstrap-v1").is_err());
    }
}
