//! Shared wire contract; clients accept additive response fields, requests are strict.
mod api;
mod encoding;
mod model;
mod validation;

pub use api::*;
pub use encoding::*;
pub use model::*;
pub use validation::*;

pub const PROTOCOL_MAJOR: u16 = {
    let bytes = env!("CARGO_PKG_VERSION_MAJOR").as_bytes();
    let mut major = 0_u16;
    let mut index = 0;
    while index < bytes.len() {
        major = match major.checked_mul(10) {
            Some(value) => match value.checked_add((bytes[index] - b'0') as u16) {
                Some(value) => value,
                None => panic!("package major exceeds protocol range"),
            },
            None => panic!("package major exceeds protocol range"),
        };
        index += 1;
    }
    major
};
pub const PROTOCOL_HEADER: &str = "Phi-Protocol";
pub const API_PREFIX: &str = "/v1";
