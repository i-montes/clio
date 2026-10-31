//! The only place that touches the ciphers: XChaCha20-Poly1305 for sealing, Argon2id to turn a
//! passphrase into the master key. A sealed blob is `nonce (24 bytes) || ciphertext+tag`.

use crate::VaultError;
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

pub(crate) const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;

pub(crate) fn random<const N: usize>() -> Result<[u8; N], VaultError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|_| VaultError::Crypto)?;
    Ok(b)
}

/// Argon2id with the OWASP minimum (19 MiB, 2 passes, 1 lane).
pub(crate) fn derive(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], VaultError> {
    let params = Params::new(19 * 1024, 2, 1, Some(KEY_LEN)).map_err(|_| VaultError::Crypto)?;
    let mut key = [0u8; KEY_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| VaultError::Crypto)?;
    Ok(key)
}

pub(crate) fn seal(key: &[u8; KEY_LEN], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>, VaultError> {
    let nonce = random::<NONCE_LEN>()?;
    let ct = XChaCha20Poly1305::new(key.into())
        .encrypt(&XNonce::from(nonce), Payload { msg: plain, aad })
        .map_err(|_| VaultError::Crypto)?;
    Ok([nonce.as_slice(), &ct].concat())
}

/// `None` when the key, the blob or the associated data is wrong: the three look the same.
pub(crate) fn open(key: &[u8; KEY_LEN], aad: &[u8], blob: &[u8]) -> Option<Vec<u8>> {
    let (nonce, ct) = blob.split_at_checked(NONCE_LEN)?;
    let nonce: [u8; NONCE_LEN] = nonce.try_into().ok()?;
    XChaCha20Poly1305::new(key.into())
        .decrypt(&XNonce::from(nonce), Payload { msg: ct, aad })
        .ok()
}
