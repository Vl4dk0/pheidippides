use crate::{
    CryptoError, DeviceKeys, Entropy, GrantKeys, VerifiedDevice, VerifiedMembership, keys::derive,
    random, signing_bytes, verify_signature,
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use phi_proto::*;
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};
use zeroize::Zeroizing;

pub struct ChannelKey {
    space_id: SpaceId,
    channel_id: ChannelId,
    epoch: Epoch,
    key: Zeroizing<[u8; 32]>,
}
impl ChannelKey {
    pub fn generate(
        space_id: SpaceId,
        channel_id: ChannelId,
        entropy: &mut impl Entropy,
    ) -> Result<Self, CryptoError> {
        Ok(Self {
            space_id,
            channel_id,
            epoch: 0,
            key: random(entropy)?,
        })
    }
    pub fn from_bytes(
        space_id: SpaceId,
        channel_id: ChannelId,
        epoch: Epoch,
        bytes: [u8; 32],
    ) -> Self {
        Self {
            space_id,
            channel_id,
            epoch,
            key: Zeroizing::new(bytes),
        }
    }
    pub fn space_id(&self) -> SpaceId {
        self.space_id
    }
    pub fn channel_id(&self) -> ChannelId {
        self.channel_id
    }
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }
    /// A signed roster binds an epoch to this key, not just to its recipients.
    pub fn commitment(&self) -> PublicKey {
        let mut hash = blake3::Hasher::new_derive_key("phi channel key commitment v2");
        hash.update(self.space_id.as_bytes());
        hash.update(self.channel_id.as_bytes());
        hash.update(&self.epoch.to_le_bytes());
        hash.update(self.key.as_ref());
        PublicKey::from_bytes(*hash.finalize().as_bytes())
    }
    pub fn export_secret(&self) -> Zeroizing<[u8; 32]> {
        self.key.clone()
    }
    pub fn rotate(&self, entropy: &mut impl Entropy) -> Result<Self, CryptoError> {
        Ok(Self {
            space_id: self.space_id,
            channel_id: self.channel_id,
            epoch: self.epoch.checked_add(1).ok_or(CryptoError::Epoch)?,
            key: random(entropy)?,
        })
    }
    fn check(&self, envelope: &MessageEnvelope) -> Result<(), CryptoError> {
        if envelope.space_id != self.space_id
            || envelope.channel_id != self.channel_id
            || envelope.epoch != self.epoch
        {
            return Err(CryptoError::Epoch);
        }
        Ok(())
    }
}
impl std::fmt::Debug for ChannelKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChannelKey([redacted])")
    }
}

fn encrypt(
    key: &[u8; 32],
    plaintext: &[u8],
    aad: &[u8],
    entropy: &mut impl Entropy,
) -> Result<Ciphertext, CryptoError> {
    let nonce = *random::<24>(entropy)?;
    let ciphertext = XChaCha20Poly1305::new(key.into())
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Authentication)?;
    Ok(Ciphertext {
        nonce: Nonce::from_bytes(nonce),
        ciphertext: Base64Bytes::new(ciphertext),
    })
}
fn decrypt(
    key: &[u8; 32],
    encrypted: &Ciphertext,
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            XNonce::from_slice(encrypted.nonce.as_bytes()),
            Payload {
                msg: encrypted.ciphertext.as_bytes(),
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Authentication)
}

/// Both public keys are KDF inputs and authenticated, preventing recipient substitution.
fn box_key(
    secret: &StaticSecret,
    other: &PublicKey,
    ephemeral: &PublicKey,
    recipient: &PublicKey,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let shared = secret.diffie_hellman(&XPublicKey::from(*other.as_bytes()));
    if !shared.was_contributory() {
        return Err(CryptoError::PublicKey);
    }
    let mut info = b"phi sealed box v1".to_vec();
    info.extend_from_slice(ephemeral.as_bytes());
    info.extend_from_slice(recipient.as_bytes());
    derive(shared.as_bytes(), &info)
}
pub fn seal(
    recipient: &PublicKey,
    plaintext: &[u8],
    context: &[u8],
    entropy: &mut impl Entropy,
) -> Result<SealedBox, CryptoError> {
    let ephemeral = StaticSecret::from(*random::<32>(entropy)?);
    let ephemeral_pub = PublicKey::from_bytes(XPublicKey::from(&ephemeral).to_bytes());
    let key = box_key(&ephemeral, recipient, &ephemeral_pub, recipient)?;
    let encrypted = encrypt(&key, plaintext, context, entropy)?;
    Ok(SealedBox {
        ephemeral_pub,
        nonce: encrypted.nonce,
        ciphertext: encrypted.ciphertext,
    })
}
pub fn unseal(
    device: &DeviceKeys,
    sealed: &SealedBox,
    context: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let key = box_key(
        &device.kex,
        &sealed.ephemeral_pub,
        &sealed.ephemeral_pub,
        &device.kex_public(),
    )?;
    decrypt(
        &key,
        &Ciphertext {
            nonce: sealed.nonce.clone(),
            ciphertext: sealed.ciphertext.clone(),
        },
        context,
    )
}
fn wrap_context(
    space_id: SpaceId,
    channel_id: ChannelId,
    epoch: Epoch,
    device_id: DeviceId,
) -> Result<Vec<u8>, CryptoError> {
    signing_bytes(
        "phi wrapped key v1",
        &(space_id, channel_id, epoch, device_id),
    )
}
pub fn wrap_channel_key(
    key: &ChannelKey,
    recipient: &VerifiedDevice,
    membership: &VerifiedMembership,
    entropy: &mut impl Entropy,
) -> Result<WrappedKey, CryptoError> {
    let cert = recipient.certificate();
    if key.commitment() != membership.data.key_commitment {
        return Err(CryptoError::Trust(
            "channel key differs from signed commitment",
        ));
    }
    if recipient.context != membership.context
        || membership.data.space_id != key.space_id
        || membership.data.channel_id != key.channel_id
        || membership.data.epoch != key.epoch
        || !membership.members.contains(&cert.data.person_id)
    {
        return Err(CryptoError::Trust(
            "key recipient outside authenticated channel membership",
        ));
    }
    if cert.data.space_id != key.space_id {
        return Err(CryptoError::Trust("recipient belongs to another space"));
    }
    let context = wrap_context(key.space_id, key.channel_id, key.epoch, cert.data.device_id)?;
    Ok(WrappedKey {
        channel_id: key.channel_id,
        epoch: key.epoch,
        device_id: cert.data.device_id,
        sealed: seal(&cert.data.kex_pub, key.key.as_ref(), &context, entropy)?,
    })
}
pub fn unwrap_channel_key(
    space_id: SpaceId,
    device_id: DeviceId,
    device: &DeviceKeys,
    wrapped: &WrappedKey,
) -> Result<ChannelKey, CryptoError> {
    wrapped.validate()?;
    if wrapped.device_id != device_id {
        return Err(CryptoError::Trust("wrapped key recipient mismatch"));
    }
    let context = wrap_context(space_id, wrapped.channel_id, wrapped.epoch, device_id)?;
    let bytes = unseal(device, &wrapped.sealed, &context)?;
    let key = bytes
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::Authentication)?;
    Ok(ChannelKey::from_bytes(
        space_id,
        wrapped.channel_id,
        wrapped.epoch,
        key,
    ))
}
fn grant_context(
    space_id: SpaceId,
    grant: &GrantKeys,
    channel_id: ChannelId,
    epoch: Epoch,
) -> Result<Vec<u8>, CryptoError> {
    signing_bytes(
        "phi grant channel key v1",
        &(space_id, grant.id(), channel_id, epoch),
    )
}
pub fn seal_grant_key(
    grant: &GrantKeys,
    channel: &ChannelKey,
    entropy: &mut impl Entropy,
) -> Result<GrantSealedKey, CryptoError> {
    let context = grant_context(channel.space_id, grant, channel.channel_id, channel.epoch)?;
    Ok(GrantSealedKey {
        channel_id: channel.channel_id,
        epoch: channel.epoch,
        sealed: encrypt(&grant.seal, channel.key.as_ref(), &context, entropy)?,
    })
}
pub fn unseal_grant_key(
    space_id: SpaceId,
    grant: &GrantKeys,
    sealed: &GrantSealedKey,
) -> Result<ChannelKey, CryptoError> {
    sealed.validate()?;
    let context = grant_context(space_id, grant, sealed.channel_id, sealed.epoch)?;
    let bytes = decrypt(&grant.seal, &sealed.sealed, &context)?;
    Ok(ChannelKey::from_bytes(
        space_id,
        sealed.channel_id,
        sealed.epoch,
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::Authentication)?,
    ))
}

#[derive(Clone, Debug)]
pub struct MessageMetadata {
    pub id: MessageId,
    pub summary_covers_seq: Option<Seq>,
    pub thread_id: Option<MessageId>,
    pub handle: Handle,
    pub from_gui: bool,
    pub mentions: Vec<Mention>,
}
fn message_aad(
    space: SpaceId,
    channel: ChannelId,
    thread: Option<MessageId>,
    epoch: Epoch,
    handle: &Handle,
    mentions: &[Mention],
) -> Result<Vec<u8>, CryptoError> {
    // Mention order is preserved: reordering signed routing data is tampering.
    let mentions: Vec<_> = mentions.iter().map(|m| (m.person_id, &m.handle)).collect();
    signing_bytes(
        "phi message aad v1",
        &(space, channel, thread, epoch, handle, mentions),
    )
}
pub fn message_signing_bytes(envelope: &MessageEnvelope) -> Result<Vec<u8>, CryptoError> {
    let mentions: Vec<_> = envelope
        .mentions
        .iter()
        .map(|m| (m.person_id, &m.handle))
        .collect();
    signing_bytes(
        "phi message v2",
        &(
            envelope.id,
            envelope.summary_covers_seq,
            envelope.space_id,
            envelope.channel_id,
            envelope.thread_id,
            envelope.epoch,
            &envelope.handle,
            envelope.from_gui,
            mentions,
            &envelope.nonce,
            &envelope.ciphertext,
        ),
    )
}
pub fn sign_message(
    keys: &DeviceKeys,
    envelope: &MessageEnvelope,
) -> Result<Signature, CryptoError> {
    envelope.validate()?;
    Ok(keys.sign(&message_signing_bytes(envelope)?))
}
pub fn verify_message_signature(
    public: &PublicKey,
    envelope: &MessageEnvelope,
    signature: &Signature,
) -> Result<(), CryptoError> {
    envelope.validate()?;
    verify_signature(public, &message_signing_bytes(envelope)?, signature)
}
pub fn encrypt_message(
    channel: &ChannelKey,
    device: &DeviceKeys,
    metadata: MessageMetadata,
    content: &MessageContent,
    entropy: &mut impl Entropy,
) -> Result<(MessageEnvelope, Signature), CryptoError> {
    content.validate()?;
    if metadata.summary_covers_seq.is_some() != content.summary.is_some()
        || metadata.thread_id.is_none() && metadata.summary_covers_seq.is_some_and(|seq| seq != 0)
    {
        return Err(CryptoError::Trust(
            "summary content and signed coverage disagree",
        ));
    }
    if metadata.thread_id.is_some() && content.title.is_some()
        || metadata.thread_id.is_none() && content.title.is_none()
    {
        return Err(CryptoError::Trust("only root messages require a title"));
    }
    let aad = message_aad(
        channel.space_id,
        channel.channel_id,
        metadata.thread_id,
        channel.epoch,
        &metadata.handle,
        &metadata.mentions,
    )?;
    let plaintext = Zeroizing::new(serde_json::to_vec(content)?);
    if plaintext.len() > MAX_CIPHERTEXT_BYTES - 16 {
        return Err(ValidationError::new(
            "content",
            "serialized content plus authentication tag exceeds 256 KiB",
        )
        .into());
    }
    let encrypted = encrypt(&channel.key, &plaintext, &aad, entropy)?;
    let envelope = MessageEnvelope {
        id: metadata.id,
        summary_covers_seq: metadata.summary_covers_seq,
        space_id: channel.space_id,
        channel_id: channel.channel_id,
        thread_id: metadata.thread_id,
        epoch: channel.epoch,
        handle: metadata.handle,
        from_gui: metadata.from_gui,
        mentions: metadata.mentions,
        nonce: encrypted.nonce,
        ciphertext: encrypted.ciphertext,
    };
    let signature = sign_message(device, &envelope)?;
    Ok((envelope, signature))
}
pub fn decrypt_message(
    channel: &ChannelKey,
    author: &VerifiedDevice,
    envelope: &MessageEnvelope,
    signature: &Signature,
) -> Result<MessageContent, CryptoError> {
    channel.check(envelope)?;
    if author.certificate().data.space_id != channel.space_id {
        return Err(CryptoError::Trust("author belongs to another space"));
    }
    verify_message_signature(&author.certificate().data.sign_pub, envelope, signature)?;
    let aad = message_aad(
        envelope.space_id,
        envelope.channel_id,
        envelope.thread_id,
        envelope.epoch,
        &envelope.handle,
        &envelope.mentions,
    )?;
    let plaintext = decrypt(
        &channel.key,
        &Ciphertext {
            nonce: envelope.nonce.clone(),
            ciphertext: envelope.ciphertext.clone(),
        },
        &aad,
    )?;
    let content: MessageContent = serde_json::from_slice(&plaintext)?;
    content.validate()?;
    if content.summary.is_some() != envelope.summary_covers_seq.is_some()
        || envelope.thread_id.is_none() && envelope.summary_covers_seq.is_some_and(|seq| seq != 0)
    {
        return Err(CryptoError::Trust(
            "summary content and signed coverage disagree",
        ));
    }
    if envelope.thread_id.is_some() && content.title.is_some()
        || envelope.thread_id.is_none() && content.title.is_none()
    {
        return Err(CryptoError::Trust("only root messages require a title"));
    }
    Ok(content)
}
pub fn encrypt_attachment(
    plaintext: &[u8],
    entropy: &mut impl Entropy,
) -> Result<(KeyBytes, Ciphertext), CryptoError> {
    let key = random::<32>(entropy)?;
    let encrypted = encrypt(&key, plaintext, b"phi attachment v1", entropy)?;
    Ok((KeyBytes::from_bytes(*key), encrypted))
}
pub fn decrypt_attachment(
    key: &KeyBytes,
    encrypted: &Ciphertext,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    decrypt(key.as_bytes(), encrypted, b"phi attachment v1")
}
pub fn encrypt_channel_metadata(
    key: &ChannelKey,
    plaintext: &[u8],
    entropy: &mut impl Entropy,
) -> Result<Ciphertext, CryptoError> {
    let context = signing_bytes(
        "phi channel metadata v1",
        &(key.space_id, key.channel_id, key.epoch),
    )?;
    let result = encrypt(&key.key, plaintext, &context, entropy)?;
    result.validate()?;
    Ok(result)
}
pub fn decrypt_channel_metadata(
    key: &ChannelKey,
    encrypted: &Ciphertext,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    encrypted.validate()?;
    let context = signing_bytes(
        "phi channel metadata v1",
        &(key.space_id, key.channel_id, key.epoch),
    )?;
    decrypt(&key.key, encrypted, &context)
}
