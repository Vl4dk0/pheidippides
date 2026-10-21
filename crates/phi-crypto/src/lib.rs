//! Pure cryptographic operations. Entropy is injected by callers; no I/O or globals.
mod keys;
mod links;
mod symmetric;
mod trust;

pub use keys::*;
pub use links::*;
pub use symmetric::*;
pub use trust::*;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error(transparent)]
    Validation(#[from] phi_proto::ValidationError),
    #[error("authentication failed")]
    Authentication,
    #[error("invalid public key")]
    PublicKey,
    #[error("entropy source failed")]
    Entropy,
    #[error("key derivation failed")]
    Derivation,
    #[error("epoch overflow or mismatch")]
    Epoch,
    #[error("invalid trust chain: {0}")]
    Trust(&'static str),
    #[error("revoked device, or not covered by its issuer's revocation history")]
    Revoked,
    #[error("invalid phi link: {0}")]
    Link(&'static str),
    #[error("serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// Callers supply OS randomness in applications, deterministic entropy in tests.
/// Returning an error avoids APIs that panic when the OS random source fails.
pub trait Entropy {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), CryptoError>;
}

fn random<const N: usize>(
    entropy: &mut impl Entropy,
) -> Result<zeroize::Zeroizing<[u8; N]>, CryptoError> {
    let mut bytes = zeroize::Zeroizing::new([0; N]);
    entropy.fill(bytes.as_mut())?;
    Ok(bytes)
}

pub fn random_id(entropy: &mut impl Entropy) -> Result<[u8; 16], CryptoError> {
    Ok(*random(entropy)?)
}

/// Domain-separated ordered JSON arrays are independent of object key ordering.
fn signing_bytes<T: serde::Serialize>(domain: &str, value: &T) -> Result<Vec<u8>, CryptoError> {
    Ok(serde_json::to_vec(&(domain, value))?)
}

impl<F> Entropy for F
where
    F: FnMut(&mut [u8]) -> Result<(), CryptoError>,
{
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), CryptoError> {
        self(bytes)
    }
}
