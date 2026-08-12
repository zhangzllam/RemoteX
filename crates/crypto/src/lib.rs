//! End-to-end session encryption built from audited `RustCrypto` primitives.

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("authentication failed")]
    AuthenticationFailed,
    #[error("invalid key material")]
    InvalidKey,
    #[error("cryptographic operation failed: {0}")]
    Other(String),
}

pub trait SessionCipher: Send + Sync {
    fn seal(&self, sequence: u64, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError>;
    fn open(&self, sequence: u64, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError>;
}

/// Separates nonces for the two data directions when a session key is reused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionDirection {
    AgentToController,
    ControllerToAgent,
}

/// XChaCha20-Poly1305 authenticated encryption for one session direction.
///
/// The 192-bit nonce consists of a direction-separated 128-bit Session ID
/// prefix followed by the message sequence number in big-endian order. A
/// sender must never reuse a sequence number with the same session key.
pub struct XChaChaSessionCipher {
    cipher: XChaCha20Poly1305,
    nonce_prefix: [u8; 16],
}

impl XChaChaSessionCipher {
    #[must_use]
    pub fn new(key: [u8; 32], session_id: [u8; 16], direction: SessionDirection) -> Self {
        let mut nonce_prefix = session_id;
        nonce_prefix[15] ^= match direction {
            SessionDirection::AgentToController => 0xa1,
            SessionDirection::ControllerToAgent => 0xc2,
        };
        Self {
            cipher: XChaCha20Poly1305::new((&key).into()),
            nonce_prefix,
        }
    }

    fn nonce(&self, sequence: u64) -> XNonce {
        let mut nonce = [0_u8; 24];
        nonce[..16].copy_from_slice(&self.nonce_prefix);
        nonce[16..].copy_from_slice(&sequence.to_be_bytes());
        nonce.into()
    }
}

impl SessionCipher for XChaChaSessionCipher {
    fn seal(&self, sequence: u64, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        self.cipher
            .encrypt(&self.nonce(sequence), plaintext)
            .map_err(|_| CryptoError::Other("encryption failed".to_owned()))
    }

    fn open(&self, sequence: u64, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        self.cipher
            .decrypt(&self.nonce(sequence), ciphertext)
            .map_err(|_| CryptoError::AuthenticationFailed)
    }
}

pub trait DeviceIdentity: Send + Sync {
    fn public_key(&self) -> &[u8];
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError>;
    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), CryptoError>;
}

#[cfg(test)]
mod tests {
    use super::{SessionCipher, SessionDirection, XChaChaSessionCipher};

    #[test]
    fn ciphertext_round_trips_for_matching_session_and_sequence() {
        let cipher =
            XChaChaSessionCipher::new([7; 32], [3; 16], SessionDirection::AgentToController);
        let sealed = cipher.seal(42, b"remote frame").expect("seal frame");

        assert_ne!(sealed, b"remote frame");
        assert_eq!(
            cipher.open(42, &sealed).expect("open frame"),
            b"remote frame"
        );
    }

    #[test]
    fn tampering_and_wrong_nonce_are_rejected() {
        let cipher =
            XChaChaSessionCipher::new([9; 32], [5; 16], SessionDirection::AgentToController);
        let mut sealed = cipher.seal(8, b"secret").expect("seal frame");
        sealed[0] ^= 1;

        assert!(cipher.open(8, &sealed).is_err());
        let valid = cipher.seal(8, b"secret").expect("seal frame");
        assert!(cipher.open(9, &valid).is_err());
    }

    #[test]
    fn opposite_directions_use_distinct_nonces() {
        let outbound =
            XChaChaSessionCipher::new([11; 32], [13; 16], SessionDirection::AgentToController);
        let inbound =
            XChaChaSessionCipher::new([11; 32], [13; 16], SessionDirection::ControllerToAgent);
        let sealed = outbound.seal(1, b"frame").expect("seal frame");

        assert!(inbound.open(1, &sealed).is_err());
    }
}
