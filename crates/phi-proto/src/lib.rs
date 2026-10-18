//! Shared wire contract; clients accept additive response fields, requests are strict.
mod api;
mod encoding;
mod model;
mod validation;

pub use api::*;
pub use encoding::*;
pub use model::*;
pub use validation::*;

pub const PROTOCOL_MAJOR: u16 = 2;
pub const PROTOCOL_HEADER: &str = "Phi-Protocol";
pub const API_PREFIX: &str = "/v1";
