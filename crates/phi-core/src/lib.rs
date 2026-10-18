//! Shared client policy, persistence and cryptographic verification for CLI and GUI.
mod client;
mod storage;
mod transport;
mod view;
pub use client::*;
pub use phi_proto::PROTOCOL_MAJOR;
pub use storage::*;
pub use transport::Error;
pub use view::*;
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub(crate) fn entropy(bytes: &mut [u8]) -> std::result::Result<(), phi_crypto::CryptoError> {
    getrandom::fill(bytes).map_err(|_| phi_crypto::CryptoError::Entropy)
}
pub(crate) fn user(message: impl Into<String>) -> Error {
    Error::User(message.into())
}
