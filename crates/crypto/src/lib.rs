//! Cryptographic capability boundaries. M0 supplies no algorithms.

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

pub trait DeviceIdentity: Send + Sync {
    fn public_key(&self) -> &[u8];
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError>;
    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), CryptoError>;
}
