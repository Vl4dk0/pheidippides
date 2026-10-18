use std::{fmt, str::FromStr};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::ValidationError;

fn decode(value: &str) -> Result<Vec<u8>, ValidationError> {
    URL_SAFE_NO_PAD.decode(value).map_err(|_| {
        ValidationError::new(
            "base64",
            "expected canonical URL-safe base64 without padding",
        )
    })
}

macro_rules! id {
    ($name:ident, $length:expr) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; $length]);
        impl $name {
            pub const fn from_bytes(bytes: [u8; $length]) -> Self {
                Self(bytes)
            }
            pub const fn as_bytes(&self) -> &[u8; $length] {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&URL_SAFE_NO_PAD.encode(self.0))
            }
        }
        impl FromStr for $name {
            type Err = ValidationError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                if value.len() != ($length * 4_usize).div_ceil(3) {
                    return Err(ValidationError::new(
                        stringify!($name),
                        concat!("expected ", stringify!($length), " bytes"),
                    ));
                }
                let bytes = decode(value)?.try_into().map_err(|_| {
                    ValidationError::new(
                        stringify!($name),
                        concat!("expected ", stringify!($length), " bytes"),
                    )
                })?;
                Ok(Self(bytes))
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer)?
                    .parse()
                    .map_err(de::Error::custom)
            }
        }
    };
}
id!(SpaceId, 16);
id!(PersonId, 16);
id!(DeviceId, 16);
id!(ChannelId, 16);
id!(MessageId, 16);
id!(BlobId, 16);
id!(GrantId, 32);

/// Fixed-size binary fields cannot bypass length checks through deserialization.
/// All copies zeroize on drop, including attachment keys carried inside content.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Zeroize, ZeroizeOnDrop)]
pub struct Base64<const N: usize>([u8; N]);
impl<const N: usize> Base64<N> {
    pub const fn from_bytes(bytes: [u8; N]) -> Self {
        Self(bytes)
    }
    pub const fn as_bytes(&self) -> &[u8; N] {
        &self.0
    }
}
impl<const N: usize> fmt::Debug for Base64<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Base64<{N}>([redacted])")
    }
}
impl<const N: usize> FromStr for Base64<N> {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != N.saturating_mul(4).div_ceil(3) {
            return Err(ValidationError::new(
                "base64",
                format!("expected {N} bytes"),
            ));
        }
        let decoded = Zeroizing::new(decode(value)?);
        if decoded.len() != N {
            return Err(ValidationError::new(
                "base64",
                format!("expected {N} bytes"),
            ));
        }
        let mut bytes = Zeroizing::new([0; N]);
        bytes.copy_from_slice(&decoded);
        Ok(Self(*bytes))
    }
}
impl<const N: usize> Serialize for Base64<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&Zeroizing::new(URL_SAFE_NO_PAD.encode(self.0)))
    }
}
impl<'de, const N: usize> Deserialize<'de> for Base64<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Zeroizing::new(String::deserialize(deserializer)?)
            .parse()
            .map_err(de::Error::custom)
    }
}
pub type PublicKey = Base64<32>;
pub type Signature = Base64<64>;
pub type Nonce = Base64<24>;
pub type KeyBytes = Base64<32>;

#[derive(Clone, Default, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Base64Bytes(Vec<u8>);
impl Base64Bytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
impl fmt::Debug for Base64Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Base64Bytes({} bytes)", self.0.len())
    }
}
impl FromStr for Base64Bytes {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(decode(value)?))
    }
}
impl Serialize for Base64Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(&self.0))
    }
}
impl<'de> Deserialize<'de> for Base64Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Zeroizing::new(String::deserialize(deserializer)?)
            .parse()
            .map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Handle(String);
impl Handle {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for Handle {
    type Error = ValidationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 40
            || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
            || !bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(b))
        {
            return Err(ValidationError::new(
                "handle",
                "expected ^[a-z0-9][a-z0-9._-]{0,39}$",
            ));
        }
        Ok(Self(value))
    }
}
impl FromStr for Handle {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.to_owned().try_into()
    }
}
impl From<Handle> for String {
    fn from(value: Handle) -> Self {
        value.0
    }
}
impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
