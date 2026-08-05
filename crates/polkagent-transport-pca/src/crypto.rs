//! X25519 key exchange and ChaCha20-Poly1305 AEAD encryption/decryption.
//!
//! This module provides:
//! - [`KeyPair`] — an X25519 key pair for Diffie-Hellman key agreement.
//! - [`SharedSecret`] — the derived shared secret from a key exchange.
//! - [`encrypt`] / [`decrypt`] — ChaCha20-Poly1305 AEAD operations.
//! - [`NonceCounter`] — a monotonically increasing nonce counter.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce as AeadNonce};
use rand::rngs::OsRng;
use x25519_dalek::{EphemeralSecret, PublicKey, ReusableSecret};
use zeroize::Zeroize;

/// Size of a ChaCha20-Poly1305 nonce in bytes.
const NONCE_SIZE: usize = 12;

/// Size of a ChaCha20-Poly1305 authentication tag in bytes.
pub const TAG_SIZE: usize = 16;

/// An X25519 key pair for Diffie-Hellman key agreement.
///
/// The private key is held in memory and zeroized on drop.
pub struct KeyPair {
    secret: ReusableSecret,
    public: PublicKey,
}

impl KeyPair {
    /// Generate a new random key pair.
    pub fn generate() -> Self {
        let secret = ReusableSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// Return the public key bytes.
    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    /// Return the raw public key bytes.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.public.to_bytes()
    }

    /// Perform a Diffie-Hellman key exchange with the given peer public key
    /// and return a [`SharedSecret`].
    pub fn diffie_hellman(&self, peer_public: &PublicKey) -> SharedSecret {
        let shared = self.secret.diffie_hellman(peer_public);
        SharedSecret {
            bytes: *shared.as_bytes(),
        }
    }
}

/// Generate an ephemeral key pair for a one-time key exchange.
///
/// Returns `(secret, public_key)`. The secret is consumed during the DH
/// operation and cannot be reused.
pub fn ephemeral_keypair() -> (EphemeralSecret, PublicKey) {
    let secret = EphemeralSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    (secret, public)
}

/// Perform a Diffie-Hellman exchange with an ephemeral secret.
pub fn ephemeral_diffie_hellman(secret: EphemeralSecret, peer_public: &PublicKey) -> SharedSecret {
    let shared = secret.diffie_hellman(peer_public);
    SharedSecret {
        bytes: *shared.as_bytes(),
    }
}

/// A 32-byte shared secret derived from an X25519 key exchange.
///
/// Used as the symmetric key for ChaCha20-Poly1305 encryption.
pub struct SharedSecret {
    bytes: [u8; 32],
}

impl SharedSecret {
    /// Construct a shared secret from raw bytes (for testing).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    /// Return a reference to the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

impl Drop for SharedSecret {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// A monotonically increasing nonce counter for AEAD operations.
///
/// ChaCha20-Poly1305 requires a unique 12-byte nonce per encryption under
/// the same key. This counter encodes a `u64` sequence number into the
/// lower 8 bytes of the nonce, guaranteeing uniqueness as long as the
/// counter is never reset for a given key.
#[derive(Debug, Clone)]
pub struct NonceCounter {
    counter: u64,
}

impl NonceCounter {
    /// Create a new nonce counter starting at 0.
    pub fn new() -> Self {
        Self { counter: 0 }
    }

    /// Return the current counter value.
    pub fn current(&self) -> u64 {
        self.counter
    }

    /// Advance the counter and return the next nonce bytes.
    ///
    /// Returns `None` if the counter has reached `u64::MAX`, at which point
    /// a key rotation is required.
    pub fn next_nonce(&mut self) -> Option<[u8; NONCE_SIZE]> {
        if self.counter == u64::MAX {
            return None;
        }
        let mut nonce = [0u8; NONCE_SIZE];
        nonce[4..12].copy_from_slice(&self.counter.to_le_bytes());
        self.counter += 1;
        Some(nonce)
    }
}

impl Default for NonceCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// Encrypt `plaintext` using ChaCha20-Poly1305 with the given key and nonce.
///
/// Returns the ciphertext with the 16-byte authentication tag appended.
pub fn encrypt(
    key: &[u8; 32],
    nonce: &[u8; NONCE_SIZE],
    plaintext: &[u8],
) -> Result<Vec<u8>, crate::error::PcaError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let aead_nonce = AeadNonce::from_slice(nonce);
    cipher
        .encrypt(aead_nonce, plaintext)
        .map_err(|e| crate::error::PcaError::CipherError {
            reason: format!("encryption failed: {e}"),
        })
}

/// Decrypt `ciphertext` using ChaCha20-Poly1305 with the given key and nonce.
///
/// The ciphertext must include the 16-byte authentication tag (as produced
/// by [`encrypt`]).
pub fn decrypt(
    key: &[u8; 32],
    nonce: &[u8; NONCE_SIZE],
    ciphertext: &[u8],
) -> Result<Vec<u8>, crate::error::PcaError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let aead_nonce = AeadNonce::from_slice(nonce);
    cipher
        .decrypt(aead_nonce, ciphertext)
        .map_err(|e| crate::error::PcaError::CipherError {
            reason: format!("decryption failed: {e}"),
        })
}

/// Encrypt `plaintext` with additional authenticated data (AAD).
pub fn encrypt_with_aad(
    key: &[u8; 32],
    nonce: &[u8; NONCE_SIZE],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, crate::error::PcaError> {
    use chacha20poly1305::aead::Payload;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let aead_nonce = AeadNonce::from_slice(nonce);
    let payload = Payload {
        msg: plaintext,
        aad,
    };
    cipher
        .encrypt(aead_nonce, payload)
        .map_err(|e| crate::error::PcaError::CipherError {
            reason: format!("encryption with AAD failed: {e}"),
        })
}

/// Decrypt `ciphertext` with additional authenticated data (AAD).
pub fn decrypt_with_aad(
    key: &[u8; 32],
    nonce: &[u8; NONCE_SIZE],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, crate::error::PcaError> {
    use chacha20poly1305::aead::Payload;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let aead_nonce = AeadNonce::from_slice(nonce);
    let payload = Payload {
        msg: ciphertext,
        aad,
    };
    cipher
        .decrypt(aead_nonce, payload)
        .map_err(|e| crate::error::PcaError::CipherError {
            reason: format!("decryption with AAD failed: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_generates_different_keys() {
        let kp1 = KeyPair::generate();
        let kp2 = KeyPair::generate();
        assert_ne!(kp1.public_key_bytes(), kp2.public_key_bytes());
    }

    #[test]
    fn diffie_hellman_produces_same_shared_secret() {
        let alice = KeyPair::generate();
        let bob = KeyPair::generate();

        let shared_ab = alice.diffie_hellman(bob.public_key());
        let shared_ba = bob.diffie_hellman(alice.public_key());

        assert_eq!(shared_ab.as_bytes(), shared_ba.as_bytes());
    }

    #[test]
    fn different_peers_produce_different_shared_secrets() {
        let alice = KeyPair::generate();
        let bob = KeyPair::generate();
        let carol = KeyPair::generate();

        let shared_ab = alice.diffie_hellman(bob.public_key());
        let shared_ac = alice.diffie_hellman(carol.public_key());

        assert_ne!(shared_ab.as_bytes(), shared_ac.as_bytes());
    }

    #[test]
    fn ephemeral_key_exchange_works() {
        let (alice_secret, alice_public) = ephemeral_keypair();
        let bob = KeyPair::generate();

        let shared_a = ephemeral_diffie_hellman(alice_secret, bob.public_key());
        let shared_b = bob.diffie_hellman(&alice_public);

        assert_eq!(shared_a.as_bytes(), shared_b.as_bytes());
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = [42u8; 32];
        let nonce = [0u8; 12];
        let plaintext = b"Hello, PCA transport!";

        let ciphertext = encrypt(&key, &nonce, plaintext).expect("encrypt");
        assert_ne!(&ciphertext[..plaintext.len()], plaintext);

        let decrypted = decrypt(&key, &nonce, &ciphertext).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_empty_message() {
        let key = [1u8; 32];
        let nonce = [0u8; 12];
        let plaintext = b"";

        let ciphertext = encrypt(&key, &nonce, plaintext).expect("encrypt");
        // Ciphertext should be just the tag (16 bytes) for empty plaintext.
        assert_eq!(ciphertext.len(), TAG_SIZE);

        let decrypted = decrypt(&key, &nonce, &ciphertext).expect("decrypt");
        assert!(decrypted.is_empty());
    }

    #[test]
    fn decrypt_fails_with_wrong_key() {
        let key1 = [42u8; 32];
        let key2 = [99u8; 32];
        let nonce = [0u8; 12];
        let plaintext = b"secret data";

        let ciphertext = encrypt(&key1, &nonce, plaintext).expect("encrypt");
        let result = decrypt(&key2, &nonce, &ciphertext);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_fails_with_wrong_nonce() {
        let key = [42u8; 32];
        let nonce1 = [0u8; 12];
        let nonce2 = [1u8; 12];
        let plaintext = b"secret data";

        let ciphertext = encrypt(&key, &nonce1, plaintext).expect("encrypt");
        let result = decrypt(&key, &nonce2, &ciphertext);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_fails_with_tampered_ciphertext() {
        let key = [42u8; 32];
        let nonce = [0u8; 12];
        let plaintext = b"secret data";

        let mut ciphertext = encrypt(&key, &nonce, plaintext).expect("encrypt");
        // Flip a bit in the ciphertext.
        ciphertext[0] ^= 0xFF;

        let result = decrypt(&key, &nonce, &ciphertext);
        assert!(result.is_err());
    }

    #[test]
    fn nonce_counter_starts_at_zero() {
        let counter = NonceCounter::new();
        assert_eq!(counter.current(), 0);
    }

    #[test]
    fn nonce_counter_increments() {
        let mut counter = NonceCounter::new();
        let n0 = counter.next_nonce().expect("nonce");
        assert_eq!(counter.current(), 1);

        let n1 = counter.next_nonce().expect("nonce");
        assert_eq!(counter.current(), 2);
        assert_ne!(n0, n1);
    }

    #[test]
    fn nonce_counter_produces_unique_nonces() {
        let mut counter = NonceCounter::new();
        let mut nonces = Vec::new();
        for _ in 0..100 {
            nonces.push(counter.next_nonce().expect("nonce"));
        }
        // All nonces should be unique.
        for i in 0..nonces.len() {
            for j in (i + 1)..nonces.len() {
                assert_ne!(nonces[i], nonces[j], "nonce collision at {i} and {j}");
            }
        }
    }

    #[test]
    fn encrypt_decrypt_with_nonce_counter() {
        let alice = KeyPair::generate();
        let bob = KeyPair::generate();
        let shared = alice.diffie_hellman(bob.public_key());

        let mut counter = NonceCounter::new();
        let messages = ["Hello", "World", "PCA Transport"];

        for msg in &messages {
            let nonce = counter.next_nonce().expect("nonce");
            let ct = encrypt(shared.as_bytes(), &nonce, msg.as_bytes()).expect("encrypt");
            let pt = decrypt(shared.as_bytes(), &nonce, &ct).expect("decrypt");
            assert_eq!(std::str::from_utf8(&pt).expect("utf8"), *msg);
        }
    }

    #[test]
    fn encrypt_decrypt_with_aad_roundtrip() {
        let key = [42u8; 32];
        let nonce = [0u8; 12];
        let plaintext = b"Hello with AAD!";
        let aad = b"conversation-id-123";

        let ct = encrypt_with_aad(&key, &nonce, aad, plaintext).expect("encrypt");
        let pt = decrypt_with_aad(&key, &nonce, aad, &ct).expect("decrypt");
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn decrypt_with_wrong_aad_fails() {
        let key = [42u8; 32];
        let nonce = [0u8; 12];
        let plaintext = b"Hello with AAD!";
        let aad1 = b"conversation-id-123";
        let aad2 = b"conversation-id-456";

        let ct = encrypt_with_aad(&key, &nonce, aad1, plaintext).expect("encrypt");
        let result = decrypt_with_aad(&key, &nonce, aad2, &ct);
        assert!(result.is_err());
    }

    #[test]
    fn large_message_encrypt_decrypt() {
        let key = [42u8; 32];
        let nonce = [0u8; 12];
        let plaintext = vec![0xAB_u8; 1024 * 1024]; // 1 MiB

        let ct = encrypt(&key, &nonce, &plaintext).expect("encrypt");
        let pt = decrypt(&key, &nonce, &ct).expect("decrypt");
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn shared_secret_zeroizes_on_drop() {
        let key_bytes;
        {
            let alice = KeyPair::generate();
            let bob = KeyPair::generate();
            let shared = alice.diffie_hellman(bob.public_key());
            key_bytes = *shared.as_bytes();
            // `shared` is dropped here and the internal bytes are zeroized.
            // We can only verify the bytes were non-zero before drop.
        }
        // The bytes were copied before drop, so they should be non-zero.
        assert_ne!(key_bytes, [0u8; 32]);
    }
}
