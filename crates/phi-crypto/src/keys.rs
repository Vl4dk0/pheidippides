use crate::{CryptoError, Entropy, random};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use phi_proto::*;
use sha2::Sha256;
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};
use zeroize::Zeroizing;

pub struct DeviceKeys {
    pub(crate) sign: SigningKey,
    pub(crate) kex: StaticSecret,
}
impl DeviceKeys {
    pub fn generate(entropy: &mut impl Entropy) -> Result<Self, CryptoError> {
        Ok(Self::from_secrets(*random(entropy)?, *random(entropy)?))
    }
    pub fn from_secrets(sign_seed: [u8; 32], kex_secret: [u8; 32]) -> Self {
        let sign_seed = Zeroizing::new(sign_seed);
        let kex_secret = Zeroizing::new(kex_secret);
        Self {
            sign: SigningKey::from_bytes(&sign_seed),
            kex: StaticSecret::from(*kex_secret),
        }
    }
    /// Explicit export is for the caller's protected key store, never wire data.
    pub fn export_secrets(&self) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
        (
            Zeroizing::new(self.sign.to_bytes()),
            Zeroizing::new(self.kex.to_bytes()),
        )
    }
    pub fn sign_public(&self) -> PublicKey {
        PublicKey::from_bytes(self.sign.verifying_key().to_bytes())
    }
    pub fn kex_public(&self) -> PublicKey {
        PublicKey::from_bytes(XPublicKey::from(&self.kex).to_bytes())
    }
    pub fn sign(&self, bytes: &[u8]) -> Signature {
        Signature::from_bytes(self.sign.sign(bytes).to_bytes())
    }
}
impl std::fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeviceKeys([redacted])")
    }
}
pub fn verify_signature(
    public: &PublicKey,
    bytes: &[u8],
    signature: &Signature,
) -> Result<(), CryptoError> {
    let key = VerifyingKey::from_bytes(public.as_bytes()).map_err(|_| CryptoError::PublicKey)?;
    if key.is_weak() {
        return Err(CryptoError::PublicKey);
    }
    key.verify_strict(
        bytes,
        &ed25519_dalek::Signature::from_bytes(signature.as_bytes()),
    )
    .map_err(|_| CryptoError::Authentication)
}
pub fn space_id(root_sign_pub: &PublicKey) -> SpaceId {
    let hash = blake3::hash(root_sign_pub.as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    SpaceId::from_bytes(bytes)
}

pub struct GrantSecret(Zeroizing<[u8; 32]>);
impl GrantSecret {
    pub fn generate(entropy: &mut impl Entropy) -> Result<Self, CryptoError> {
        Ok(Self(random(entropy)?))
    }
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
    pub fn derive(&self) -> Result<GrantKeys, CryptoError> {
        let id = Zeroizing::new(blake3::derive_key("phi grant id", self.0.as_ref()));
        let sign = derive(self.0.as_ref(), b"phi grant sign")?;
        let seal = derive(self.0.as_ref(), b"phi grant seal")?;
        Ok(GrantKeys {
            id: GrantId::from_bytes(*id),
            sign: SigningKey::from_bytes(&sign),
            seal,
        })
    }
}
impl std::fmt::Debug for GrantSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GrantSecret([redacted])")
    }
}
pub struct GrantKeys {
    pub(crate) id: GrantId,
    pub(crate) sign: SigningKey,
    pub(crate) seal: Zeroizing<[u8; 32]>,
}
impl GrantKeys {
    pub fn id(&self) -> GrantId {
        self.id
    }
    pub fn public(&self) -> PublicKey {
        PublicKey::from_bytes(self.sign.verifying_key().to_bytes())
    }
    pub fn sign(&self, bytes: &[u8]) -> Signature {
        Signature::from_bytes(self.sign.sign(bytes).to_bytes())
    }
}
impl std::fmt::Debug for GrantKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GrantKeys([redacted])")
    }
}
pub(crate) fn derive(secret: &[u8], info: &[u8]) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let mut key = Zeroizing::new([0; 32]);
    Hkdf::<Sha256>::new(None, secret)
        .expand(info, key.as_mut())
        .map_err(|_| CryptoError::Derivation)?;
    Ok(key)
}
