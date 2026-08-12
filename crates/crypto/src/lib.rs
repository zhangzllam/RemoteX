//! End-to-end session encryption built from audited `RustCrypto` primitives.

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
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

/// Persistent Ed25519 identity owned by one enrolled Agent device.
pub struct Ed25519DeviceIdentity {
    signing_key: SigningKey,
    public_key: [u8; 32],
}

impl Ed25519DeviceIdentity {
    #[must_use]
    pub fn generate() -> Self {
        let signing_key = SigningKey::from_bytes(&rand::random());
        Self {
            public_key: signing_key.verifying_key().to_bytes(),
            signing_key,
        }
    }

    #[must_use]
    pub fn from_secret_bytes(secret: [u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(&secret);
        Self {
            public_key: signing_key.verifying_key().to_bytes(),
            signing_key,
        }
    }

    #[must_use]
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    #[must_use]
    pub fn public_bytes(&self) -> [u8; 32] {
        self.public_key
    }
}

impl DeviceIdentity for Ed25519DeviceIdentity {
    fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Ok(self.signing_key.sign(message).to_bytes().to_vec())
    }

    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), CryptoError> {
        verify_device_signature(&self.public_bytes(), message, signature)
    }
}

pub fn verify_device_signature(
    public_key: &[u8; 32],
    message: &[u8],
    signature: &[u8],
) -> Result<(), CryptoError> {
    let verifying_key =
        VerifyingKey::from_bytes(public_key).map_err(|_| CryptoError::InvalidKey)?;
    let signature =
        Signature::from_slice(signature).map_err(|_| CryptoError::AuthenticationFailed)?;
    verifying_key
        .verify(message, &signature)
        .map_err(|_| CryptoError::AuthenticationFailed)
}

/// Domain-separated bytes signed for authenticated control-plane device calls.
#[must_use]
pub fn device_auth_message(
    action: &str,
    device_id: &str,
    timestamp_ms: u64,
    nonce: u64,
) -> Vec<u8> {
    let mut message = b"RemoteX-device-auth-v1\0".to_vec();
    message.extend_from_slice(action.as_bytes());
    message.push(0);
    message.extend_from_slice(device_id.as_bytes());
    message.extend_from_slice(&timestamp_ms.to_be_bytes());
    message.extend_from_slice(&nonce.to_be_bytes());
    message
}

/// Server-side key wrapping for short-lived Agent tokens and Session keys.
pub struct ServerSecretBox {
    cipher: XChaCha20Poly1305,
}

impl ServerSecretBox {
    #[must_use]
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            cipher: XChaCha20Poly1305::new((&key).into()),
        }
    }

    pub fn seal(&self, secret: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let nonce_bytes: [u8; 24] = rand::random();
        let ciphertext = self
            .cipher
            .encrypt(XNonce::from_slice(&nonce_bytes), secret)
            .map_err(|_| CryptoError::Other("secret wrapping failed".to_owned()))?;
        let mut wrapped = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
        wrapped.extend_from_slice(&nonce_bytes);
        wrapped.extend_from_slice(&ciphertext);
        Ok(wrapped)
    }

    pub fn open(&self, wrapped: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if wrapped.len() < 24 {
            return Err(CryptoError::AuthenticationFailed);
        }
        self.cipher
            .decrypt(XNonce::from_slice(&wrapped[..24]), &wrapped[24..])
            .map_err(|_| CryptoError::AuthenticationFailed)
    }
}

#[must_use]
pub fn secret_hash(secret: &[u8]) -> [u8; 32] {
    Sha256::digest(secret).into()
}

#[cfg(test)]
mod tests {
    use super::{
        DeviceIdentity, Ed25519DeviceIdentity, ServerSecretBox, SessionCipher, SessionDirection,
        XChaChaSessionCipher, device_auth_message, secret_hash, verify_device_signature,
    };

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

    #[test]
    fn device_identity_signs_domain_separated_control_requests() {
        let identity = Ed25519DeviceIdentity::from_secret_bytes([42; 32]);
        let message = device_auth_message("heartbeat", "825371946", 123, 7);
        let signature = identity.sign(&message).expect("sign request");
        verify_device_signature(&identity.public_bytes(), &message, &signature)
            .expect("verify signature");
        assert!(
            verify_device_signature(
                &identity.public_bytes(),
                &device_auth_message("claim", "825371946", 123, 7),
                &signature,
            )
            .is_err()
        );
    }

    #[test]
    fn server_secret_box_wraps_session_material() {
        let secret_box = ServerSecretBox::new([9; 32]);
        let wrapped = secret_box.seal(b"session secret").expect("wrap secret");
        assert_ne!(wrapped, b"session secret");
        assert_eq!(
            secret_box.open(&wrapped).expect("unwrap secret"),
            b"session secret"
        );
        assert_eq!(secret_hash(b"token"), secret_hash(b"token"));
    }
}
