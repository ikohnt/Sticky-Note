//! Local, at-rest encryption for password-protected notes.
//!
//! Design (encrypt-at-rest, the honest option from `FEATURE_BACKLOG.md` — not
//! gate-only):
//! * A single **master password** derives a 32-byte key via **Argon2id**.
//! * Protected notes' content is sealed with **AES-256-GCM** (authenticated, so
//!   tampering is detected), a fresh random 12-byte nonce per encryption.
//! * The password itself is never stored. A small **verifier** — a known token
//!   sealed with the derived key — lets us check an entered password without
//!   keeping the password or the key on disk.
//!
//! Everything here is offline and dependency-light; there is no network path and
//! the plaintext of a protected note never touches disk.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::Argon2;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// Token sealed under the derived key to verify a password on unlock.
const VERIFY_TOKEN: &[u8] = b"sticky-notes-verify-v1";
/// Salt length in bytes (Argon2 requires >= 8).
const SALT_LEN: usize = 16;
/// AES-GCM nonce length in bytes.
const NONCE_LEN: usize = 12;
/// Derived key length in bytes (AES-256).
const KEY_LEN: usize = 32;

/// Errors from key derivation or (de)serialization of encrypted content.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// Key derivation failed (e.g. bad Argon2 parameters).
    #[error("key derivation failed")]
    Kdf,
    /// Encryption failed.
    #[error("encryption failed")]
    Encrypt,
    /// Decryption or authentication failed (wrong key or tampered data).
    #[error("decryption failed")]
    Decrypt,
    /// A stored base64 blob could not be decoded.
    #[error("malformed encrypted data")]
    Malformed,
    /// The operation needs an unlocked session key but none is set.
    #[error("store is locked")]
    Locked,
}

/// A random byte array of length `N`, from the OS CSPRNG.
fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|_| CryptoError::Kdf)?;
    Ok(buf)
}

/// Derive a 32-byte key from `password` and `salt` using Argon2id.
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], CryptoError> {
    let mut key = [0u8; KEY_LEN];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| CryptoError::Kdf)?;
    Ok(key)
}

/// Encrypt `plaintext` with `key`, returning a self-describing sealed blob.
pub fn seal(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Sealed, CryptoError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce_bytes: [u8; NONCE_LEN] = random()?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::Encrypt)?;
    Ok(Sealed {
        nonce: STANDARD.encode(nonce_bytes),
        ciphertext: STANDARD.encode(ciphertext),
    })
}

/// Decrypt a sealed blob with `key`. Fails if the key is wrong or data tampered.
pub fn open(key: &[u8; KEY_LEN], sealed: &Sealed) -> Result<Vec<u8>, CryptoError> {
    let nonce_bytes = STANDARD
        .decode(&sealed.nonce)
        .map_err(|_| CryptoError::Malformed)?;
    let ciphertext = STANDARD
        .decode(&sealed.ciphertext)
        .map_err(|_| CryptoError::Malformed)?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(CryptoError::Malformed);
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(&nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| CryptoError::Decrypt)
}

/// Seal `plaintext` into a compact binary blob: `nonce (12 bytes) || ciphertext`.
/// Used for attachment files, where a JSON envelope would be wasteful.
pub fn seal_bytes(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce_bytes: [u8; NONCE_LEN] = random()?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::Encrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Open a compact binary blob produced by [`seal_bytes`].
pub fn open_bytes(key: &[u8; KEY_LEN], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if data.len() < NONCE_LEN {
        return Err(CryptoError::Malformed);
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::Decrypt)
}

/// An AES-GCM sealed blob: base64 nonce + base64 ciphertext. Serialized inline
/// with a protected note (its `content` becomes the base64 ciphertext).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sealed {
    /// Base64-encoded 12-byte nonce.
    pub nonce: String,
    /// Base64-encoded ciphertext (includes the GCM auth tag).
    pub ciphertext: String,
}

/// On-disk master credential (in `master.json`): the KDF salt and a sealed
/// verifier token. Never contains the password or the derived key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MasterCred {
    /// Base64-encoded Argon2 salt.
    salt: String,
    /// The [`VERIFY_TOKEN`] sealed under the derived key.
    verifier: Sealed,
}

impl MasterCred {
    /// Create a fresh credential for `password`, returning it together with the
    /// derived session key so the caller starts out unlocked.
    pub fn create(password: &str) -> Result<(Self, [u8; KEY_LEN]), CryptoError> {
        let salt: [u8; SALT_LEN] = random()?;
        let key = derive_key(password, &salt)?;
        let verifier = seal(&key, VERIFY_TOKEN)?;
        Ok((
            MasterCred {
                salt: STANDARD.encode(salt),
                verifier,
            },
            key,
        ))
    }

    /// Verify `password` against this credential. Returns the derived session key
    /// on success, or `None` if the password is wrong.
    pub fn unlock(&self, password: &str) -> Result<Option<[u8; KEY_LEN]>, CryptoError> {
        let salt = STANDARD
            .decode(&self.salt)
            .map_err(|_| CryptoError::Malformed)?;
        let key = derive_key(password, &salt)?;
        match open(&key, &self.verifier) {
            Ok(token) if token == VERIFY_TOKEN => Ok(Some(key)),
            Ok(_) => Ok(None),
            // A decrypt failure means the wrong password (not a hard error).
            Err(CryptoError::Decrypt) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trips() {
        let (_cred, key) = MasterCred::create("hunter2").unwrap();
        let sealed = seal(&key, b"secret note body").unwrap();
        assert_ne!(sealed.ciphertext, STANDARD.encode(b"secret note body"));
        assert_eq!(open(&key, &sealed).unwrap(), b"secret note body");
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let (_c1, k1) = MasterCred::create("password-one").unwrap();
        let (_c2, k2) = MasterCred::create("password-two").unwrap();
        let sealed = seal(&k1, b"top secret").unwrap();
        assert!(matches!(open(&k2, &sealed), Err(CryptoError::Decrypt)));
    }

    #[test]
    fn master_cred_unlocks_with_right_password_only() {
        let (cred, key) = MasterCred::create("correct horse").unwrap();
        let unlocked = cred.unlock("correct horse").unwrap();
        assert_eq!(unlocked, Some(key));
        assert_eq!(cred.unlock("wrong password").unwrap(), None);
    }

    #[test]
    fn nonce_differs_per_seal() {
        let (_c, key) = MasterCred::create("pw").unwrap();
        let a = seal(&key, b"same").unwrap();
        let b = seal(&key, b"same").unwrap();
        assert_ne!(a.nonce, b.nonce, "each seal must use a fresh nonce");
    }

    #[test]
    fn seal_bytes_round_trips_and_detects_tampering() {
        let (_c, key) = MasterCred::create("pw").unwrap();
        let blob = seal_bytes(&key, b"\x89PNG binary image data").unwrap();
        assert_eq!(open_bytes(&key, &blob).unwrap(), b"\x89PNG binary image data");

        // Flip a ciphertext byte -> authentication fails.
        let mut tampered = blob.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        assert!(matches!(open_bytes(&key, &tampered), Err(CryptoError::Decrypt)));
    }
}
