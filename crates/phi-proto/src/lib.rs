//! Shared wire contract; clients accept additive response fields, requests are strict.
mod api;
mod encoding;
mod model;
mod validation;

pub use api::*;
pub use encoding::*;
pub use model::*;
pub use validation::*;

/// Peers talk only when this matches. It is the breaking part of the package version, so a
/// wire break can't be forgotten: under 0.x the minor breaks, from 1.0 the major does.
pub const PROTOCOL: &str = if is_zero(env!("CARGO_PKG_VERSION_MAJOR")) {
    concat!("0.", env!("CARGO_PKG_VERSION_MINOR"))
} else {
    env!("CARGO_PKG_VERSION_MAJOR")
};
const fn is_zero(major: &str) -> bool {
    major.len() == 1 && major.as_bytes()[0] == b'0'
}
pub const PROTOCOL_HEADER: &str = "Phi-Protocol";
pub const API_PREFIX: &str = "/v1";
