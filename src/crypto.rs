#![allow(dead_code)]

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use rand::RngCore;

/// Size of the AES-256 key in bytes.
const KEY_SIZE: usize = 32;
/// Size of the GCM nonce in bytes.
const NONCE_SIZE: usize = 12;

/// Errors that can occur during encryption or decryption.
#[derive(Debug)]
pub enum CryptoError {
    /// The ciphertext was shorter than the nonce.
    MalformedCiphertext,
    /// AES-GCM decryption failed (wrong key or corrupted data).
    DecryptionFailed,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::MalformedCiphertext => write!(f, "malformed ciphertext"),
            CryptoError::DecryptionFailed => write!(f, "decryption failed"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Generate a random 256-bit encryption key.
pub fn generate_key() -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

/// Encrypt a plaintext secret using AES-256-GCM.
///
/// Returns `ciphertext || nonce` (nonce is prepended).
pub fn encrypt(key: &[u8; KEY_SIZE], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("valid key size");
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .expect("encryption failed");

    // Prepend nonce to ciphertext: [nonce (12 bytes) | ciphertext]
    let mut blob = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    blob
}

/// Decrypt a blob produced by [`encrypt`].
///
/// Expects `ciphertext || nonce` format (nonce prepended).
pub fn decrypt(key: &[u8; KEY_SIZE], blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if blob.len() < NONCE_SIZE {
        return Err(CryptoError::MalformedCiphertext);
    }

    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_SIZE);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(key).expect("valid key size");
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::DecryptionFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = generate_key();
        let plaintext = b"the secret password";

        let blob = encrypt(&key, plaintext);
        assert!(blob.len() > plaintext.len()); // nonce + ciphertext overhead

        let decrypted = decrypt(&key, &blob).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let key1 = generate_key();
        let key2 = generate_key();
        let plaintext = b"secret";

        let blob = encrypt(&key1, plaintext);
        let result = decrypt(&key2, &blob);
        assert!(matches!(result, Err(CryptoError::DecryptionFailed)));
    }

    #[test]
    fn decrypt_truncated_fails() {
        let key = generate_key();
        let blob = vec![0u8; 5]; // too short to contain a nonce
        let result = decrypt(&key, &blob);
        assert!(matches!(result, Err(CryptoError::MalformedCiphertext)));
    }
}
