use crate::*;
use std::collections::BTreeSet;

pub const MAX_CIPHERTEXT_BYTES: usize = 256 * 1024;
pub const DEFAULT_MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
pub const DEFAULT_SPACE_QUOTA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_GRANT_LIFETIME_SECS: u64 = 7 * 24 * 60 * 60;
pub const MAX_DEVICES: usize = 1024;
pub const MAX_MEMBERS: usize = 1024;
pub const MAX_MENTIONS: usize = 128;
pub const MAX_ATTACHMENTS: usize = 32;
pub const MAX_PAGE_SIZE: u32 = 1000;
pub const MAX_WAIT_SECONDS: u16 = 900;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {message}")]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}
impl ValidationError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}
pub trait Validate {
    fn validate(&self) -> Result<(), ValidationError>;
}
fn check(valid: bool, field: &str, message: &str) -> Result<(), ValidationError> {
    if valid {
        Ok(())
    } else {
        Err(ValidationError::new(field, message))
    }
}
fn encrypted(bytes: &Base64Bytes, max: usize) -> Result<(), ValidationError> {
    check(
        (16..=max).contains(&bytes.len()),
        "ciphertext",
        &format!("expected 16..={max} decoded bytes"),
    )
}
fn members(ids: &[PersonId]) -> Result<(), ValidationError> {
    check(
        !ids.is_empty() && ids.len() <= MAX_MEMBERS,
        "members",
        "expected 1..=1024 members",
    )?;
    check(
        ids.iter().collect::<BTreeSet<_>>().len() == ids.len(),
        "members",
        "duplicate person",
    )
}
impl Validate for Ciphertext {
    fn validate(&self) -> Result<(), ValidationError> {
        encrypted(&self.ciphertext, MAX_CIPHERTEXT_BYTES)
    }
}
impl Validate for SealedBox {
    fn validate(&self) -> Result<(), ValidationError> {
        encrypted(&self.ciphertext, MAX_CIPHERTEXT_BYTES)
    }
}
impl Validate for DeviceCertificate {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            self.data.sign_pub.as_bytes() != &[0; 32],
            "sign_pub",
            "zero key is invalid",
        )?;
        check(
            self.data.kex_pub.as_bytes() != &[0; 32],
            "kex_pub",
            "zero key is invalid",
        )
    }
}
impl Validate for GrantRecord {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            self.data.grant_pub.as_bytes() != &[0; 32],
            "grant_pub",
            "zero key is invalid",
        )?;
        check(
            self.data
                .expires_at
                .is_none_or(|end| end > self.data.created_at),
            "expires_at",
            "must follow created_at",
        )?;
        check(
            self.data.kind != GrantKind::Invite || self.data.expires_at.is_some(),
            "expires_at",
            "invite grants must expire",
        )
    }
}
impl Validate for Revocation {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}
impl Validate for WrappedKey {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            self.sealed.ciphertext.len() == 48,
            "wrapped_key",
            "expected encrypted 32-byte channel key",
        )
    }
}
impl Validate for GrantSealedKey {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            self.sealed.ciphertext.len() == 48,
            "sealed_key",
            "expected encrypted 32-byte channel key",
        )
    }
}
impl Validate for MessageEnvelope {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            self.thread_id.is_some() || self.summary_covers_seq.is_none_or(|seq| seq == 0),
            "summary_covers_seq",
            "a root can cover only zero existing sequences",
        )?;
        encrypted(&self.ciphertext, MAX_CIPHERTEXT_BYTES)?;
        check(
            self.mentions.len() <= MAX_MENTIONS,
            "mentions",
            "at most 128 mentions",
        )?;
        let mut seen = BTreeSet::new();
        check(
            self.mentions
                .iter()
                .all(|m| seen.insert((m.person_id, m.handle.clone()))),
            "mentions",
            "duplicate mention",
        )
    }
}
impl Validate for Attachment {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            !self.name.is_empty()
                && self.name.len() <= 255
                && !self.name.contains(['/', '\\', '\0'])
                && self.name != "."
                && self.name != "..",
            "attachment.name",
            "expected safe filename of 1..=255 bytes",
        )?;
        single_line(&self.name, "attachment.name")?;
        single_line(&self.mime, "attachment.mime")?;
        check(
            !self.mime.is_empty()
                && self.mime.len() <= 255
                && !self.mime.chars().any(char::is_control),
            "attachment.mime",
            "expected MIME type of 1..=255 bytes",
        )
    }
}
impl MessageContent {
    /// File sizes are deployment policy; wire validation must not freeze the default.
    pub fn validate_with_attachment_limit(&self, max_bytes: u64) -> Result<(), ValidationError> {
        self.validate()?;
        check(
            self.attachments.iter().all(|a| a.size <= max_bytes),
            "attachments.size",
            "attachment exceeds configured size limit",
        )
    }
}
impl Validate for MessageContent {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(title) = &self.title {
            single_line(title, "title")?;
        }
        if let Some(summary) = &self.summary {
            check(
                !summary.chars().any(|c| {
                    (c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
                        || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                }),
                "summary",
                "unsupported control or bidi formatting",
            )?;
        }
        single_line(&self.display_name, "display_name")?;
        check(
            self.body.len() <= MAX_CIPHERTEXT_BYTES,
            "body",
            "at most 256 KiB",
        )?;
        check(
            self.title
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 1024),
            "title",
            "expected 1..=1024 bytes",
        )?;
        check(
            self.summary
                .as_ref()
                .is_none_or(|s| s.len() <= MAX_CIPHERTEXT_BYTES),
            "summary",
            "at most 256 KiB",
        )?;
        check(
            self.display_name.len() <= 255,
            "display_name",
            "at most 255 bytes",
        )?;
        check(
            self.attachments.len() <= MAX_ATTACHMENTS,
            "attachments",
            "at most 32 attachments",
        )?;
        for attachment in &self.attachments {
            attachment.validate()?;
        }
        Ok(())
    }
}
impl Validate for SpaceSettings {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            self.summary_every > 0,
            "summary_every",
            "must be a positive integer",
        )?;
        check(
            self.rate_limits.messages_per_minute > 0 && self.rate_limits.messages_per_hour > 0,
            "rate_limits",
            "limits must be positive",
        )
    }
}
impl Validate for SettingsChange {
    fn validate(&self) -> Result<(), ValidationError> {
        self.data.settings.validate()
    }
}
impl Validate for Reaction {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}
impl Validate for CreateSpaceRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.root_cert.validate()?;
        self.all_channel.validate()?;
        self.self_channel.validate()?;
        let root = &self.root_cert.data;
        check(
            root.issuer
                == CertificateIssuer::Device {
                    device_id: root.device_id,
                },
            "root_cert.issuer",
            "root must be self-signed",
        )?;
        check(
            self.all_channel.space_id == root.space_id
                && self.self_channel.space_id == root.space_id,
            "space_id",
            "channels must belong to root space",
        )?;
        check(
            self.all_channel.kind == ChannelKind::All
                && self.self_channel.kind == ChannelKind::SelfChannel
                && self.all_channel.id != self.self_channel.id,
            "channels",
            "expected distinct all and self channels",
        )?;
        check(
            self.all_channel.members == [root.person_id]
                && self.self_channel.members == [root.person_id],
            "members",
            "initial channels contain only the creator",
        )
    }
}
impl Validate for UpdateSettingsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.change.validate()
    }
}
impl Validate for CreateGrantRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.record.validate()?;
        check(
            !self.sealed_keys.is_empty() && self.sealed_keys.len() <= MAX_DEVICES,
            "sealed_keys",
            "expected 1..=1024 keys",
        )?;
        let mut seen = BTreeSet::new();
        for key in &self.sealed_keys {
            key.validate()?;
            check(
                seen.insert((key.channel_id, key.epoch)),
                "sealed_keys",
                "duplicate channel epoch",
            )?;
        }
        Ok(())
    }
}
impl Validate for RedeemGrantRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.certificate.validate()?;
        check(
            matches!(
                self.certificate.data.issuer,
                CertificateIssuer::Grant { .. }
            ),
            "certificate.issuer",
            "redemption requires a grant issuer",
        )
    }
}
impl Validate for CreateRevocationRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.revocation.validate()
    }
}
impl Validate for CreateChannelRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.meta.validate()?;
        members(&self.members)?;
        check(
            match self.kind {
                ChannelKind::Dm => self.members.len() == 2,
                ChannelKind::SelfChannel => self.members.len() == 1,
                _ => true,
            },
            "members",
            "dm requires two persons; self requires one",
        )?;
        self.membership.validate()?;
        let signed = &self.membership.data;
        check(
            signed.previous.is_none()
                && signed.space_id == self.space_id
                && signed.channel_id == self.id
                && signed.kind == self.kind
                && signed.epoch == 0
                && signed.members == self.members,
            "membership",
            "signed channel fields mismatch",
        )?;
        PutKeysRequest {
            epoch: 0,
            wrapped_keys: self.wrapped_keys.clone(),
        }
        .validate()?;
        check(
            self.wrapped_keys.iter().all(|k| k.channel_id == self.id),
            "wrapped_keys",
            "channel id mismatch",
        )
    }
}
impl Validate for UpdateChannelRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if !self.wrapped_keys.is_empty() {
            PutKeysRequest {
                epoch: self.epoch,
                wrapped_keys: self.wrapped_keys.clone(),
            }
            .validate()?;
        }
        self.meta.validate()?;
        self.membership.validate()?;
        check(
            self.membership.data.members == self.members
                && self.membership.data.epoch == self.epoch,
            "membership",
            "signed channel fields mismatch",
        )?;
        members(&self.members)
    }
}
impl Validate for PutKeysRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            !self.wrapped_keys.is_empty() && self.wrapped_keys.len() <= MAX_DEVICES,
            "wrapped_keys",
            "expected 1..=1024 keys",
        )?;
        let mut seen = BTreeSet::new();
        let channel = self.wrapped_keys[0].channel_id;
        for key in &self.wrapped_keys {
            key.validate()?;
            check(
                key.epoch == self.epoch && key.channel_id == channel,
                "wrapped_keys",
                "epoch or channel mismatch",
            )?;
            check(
                seen.insert(key.device_id),
                "wrapped_keys",
                "duplicate device",
            )?;
        }
        Ok(())
    }
}
impl Validate for SendMessageRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.envelope.validate()?;
        check(
            self.id == self.envelope.id
                && self.has_summary == self.envelope.summary_covers_seq.is_some(),
            "envelope",
            "signed message id or summary claim mismatch",
        )?;
        check(
            self.blob_ids.len() <= MAX_ATTACHMENTS
                && self.blob_ids.iter().collect::<BTreeSet<_>>().len() == self.blob_ids.len(),
            "blob_ids",
            "at most 32 distinct blob references",
        )
    }
}
impl Validate for MessagesQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            self.limit.is_none_or(|n| (1..=MAX_PAGE_SIZE).contains(&n)),
            "limit",
            "expected 1..=1000",
        )
    }
}
impl Validate for ReactionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.reaction.validate()
    }
}
impl Validate for PutCursorRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}
impl Validate for CountsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}
impl Validate for EventsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            self.timeout.is_none_or(|s| s <= MAX_WAIT_SECONDS),
            "timeout",
            "at most 900 seconds",
        )
    }
}
impl UploadBlobRequest {
    pub fn validate_with_limit(&self, max_plaintext_bytes: u64) -> Result<(), ValidationError> {
        check(
            self.ciphertext.len() >= 16
                && (self.ciphertext.len() as u64).saturating_sub(16) <= max_plaintext_bytes,
            "ciphertext",
            "blob exceeds configured size limit or lacks authentication tag",
        )
    }
}
impl Validate for UploadBlobRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check(
            self.ciphertext.len() >= 16,
            "ciphertext",
            "blob lacks authentication tag",
        )
    }
}

// The same signed objects appear in additive responses and strict requests.
// Comparing parsed field names against their typed representation preserves both
// contracts without maintaining a second copy of every nested wire schema.
pub(crate) fn strict<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    use serde::{Deserialize, de::Error};
    let original = serde_json::Value::deserialize(deserializer)?;
    let parsed = T::deserialize(&original).map_err(D::Error::custom)?;
    let schema = serde_json::to_value(&parsed).map_err(D::Error::custom)?;
    reject_unknown(&original, &schema, "request").map_err(D::Error::custom)?;
    Ok(parsed)
}
fn reject_unknown(
    original: &serde_json::Value,
    schema: &serde_json::Value,
    path: &str,
) -> Result<(), ValidationError> {
    match (original, schema) {
        (serde_json::Value::Object(input), serde_json::Value::Object(known)) => {
            for (key, value) in input {
                let path = format!("{path}.{key}");
                let expected = known
                    .get(key)
                    .ok_or_else(|| ValidationError::new(&path, "unknown field"))?;
                reject_unknown(value, expected, &path)?;
            }
        }
        (serde_json::Value::Array(input), serde_json::Value::Array(known)) => {
            for (index, (value, expected)) in input.iter().zip(known).enumerate() {
                reject_unknown(value, expected, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn single_line(text: &str, field: &str) -> Result<(), ValidationError> {
    check(!text.chars().any(|c| c.is_control() || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{2028}' | '\u{2029}')), field, "control characters, line separators and bidi controls are forbidden")
}
impl Validate for ChannelMembership {
    fn validate(&self) -> Result<(), ValidationError> {
        members(&self.data.members)?;
        check(
            match self.data.kind {
                ChannelKind::Dm => self.data.members.len() == 2,
                ChannelKind::SelfChannel => self.data.members.len() == 1,
                _ => true,
            },
            "membership.members",
            "dm requires two persons; self requires one",
        )
    }
}
