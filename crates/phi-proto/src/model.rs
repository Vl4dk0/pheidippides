use crate::*;
use serde::{Deserialize, Serialize};

pub type Timestamp = u64;
pub type Seq = u64;
pub type Epoch = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantKind {
    Invite,
    Enroll,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    All,
    Group,
    Dm,
    #[serde(rename = "self")]
    SelfChannel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactionKind {
    Up,
    Down,
    Ack,
    Claim,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryMode {
    #[default]
    Required,
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CertificateIssuer {
    Device { device_id: DeviceId },
    Grant { grant_id: GrantId },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCertificateData {
    pub space_id: SpaceId,
    pub person_id: PersonId,
    pub device_id: DeviceId,
    pub sign_pub: PublicKey,
    pub kex_pub: PublicKey,
    pub created_at: Timestamp,
    pub issuer: CertificateIssuer,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCertificate {
    pub data: DeviceCertificateData,
    pub signature: Signature,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRecordData {
    pub space_id: SpaceId,
    pub grant_id: GrantId,
    pub grant_pub: PublicKey,
    pub kind: GrantKind,
    pub person_id: PersonId,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
    pub issuer_device: DeviceId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRecord {
    pub data: GrantRecordData,
    pub signature: Signature,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RevocationTarget {
    Device { device_id: DeviceId },
    Person { person_id: PersonId },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationData {
    pub space_id: SpaceId,
    pub target: RevocationTarget,
    pub issuer_device: DeviceId,
    pub created_at: Timestamp,
    /// Root over the signatures of everything the target had signed when the server froze it.
    pub history: HistoryRoot,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revocation {
    pub data: RevocationData,
    pub signature: Signature,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ciphertext {
    pub nonce: Nonce,
    pub ciphertext: Base64Bytes,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedBox {
    pub ephemeral_pub: PublicKey,
    pub nonce: Nonce,
    pub ciphertext: Base64Bytes,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrappedKey {
    pub channel_id: ChannelId,
    pub epoch: Epoch,
    pub device_id: DeviceId,
    pub sealed: SealedBox,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantSealedKey {
    pub channel_id: ChannelId,
    pub epoch: Epoch,
    pub sealed: Ciphertext,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mention {
    pub person_id: PersonId,
    pub handle: Option<Handle>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEnvelope {
    pub id: MessageId,
    /// Coverage of already assigned thread sequences; this message is included by id.
    pub summary_covers_seq: Option<Seq>,
    pub space_id: SpaceId,
    pub channel_id: ChannelId,
    pub thread_id: Option<MessageId>,
    pub epoch: Epoch,
    pub handle: Handle,
    /// Declared by the sending client and trusted like the handle: it marks a message typed by a
    /// person in the GUI. A custom client can forge it; the signature only stops the host.
    pub from_gui: bool,
    pub mentions: Vec<Mention>,
    pub nonce: Nonce,
    pub ciphertext: Base64Bytes,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub envelope: MessageEnvelope,
    pub author_person: PersonId,
    pub author_device: DeviceId,
    pub signature: Signature,
    pub seq: Seq,
    pub created_at: Timestamp,
    pub has_summary: bool,
    pub summary_covers_seq: Option<Seq>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub blob_id: BlobId,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub key: KeyBytes,
    pub nonce: Nonce,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageContent {
    pub title: Option<String>,
    pub body: String,
    pub summary: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub display_name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionData {
    pub space_id: SpaceId,
    pub message_id: MessageId,
    pub person_id: PersonId,
    pub device_id: DeviceId,
    pub handle: Handle,
    pub kind: ReactionKind,
    pub created_at: Timestamp,
    /// Removal is signed too, so an unclaim cannot be forged by the host.
    pub remove: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    pub data: ReactionData,
    pub signature: Signature,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadStateData {
    pub space_id: SpaceId,
    pub thread_id: MessageId,
    pub person_id: PersonId,
    pub device_id: DeviceId,
    pub handle: Handle,
    pub closed: bool,
    pub created_at: Timestamp,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadState {
    pub data: ThreadStateData,
    pub signature: Signature,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadDeletionData {
    pub space_id: SpaceId,
    pub thread_id: MessageId,
    pub person_id: PersonId,
    pub device_id: DeviceId,
    pub handle: Handle,
    pub created_at: Timestamp,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadDeletion {
    pub data: ThreadDeletionData,
    pub signature: Signature,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimits {
    pub messages_per_minute: u32,
    pub messages_per_hour: u32,
}
impl Default for RateLimits {
    fn default() -> Self {
        Self {
            messages_per_minute: 30,
            messages_per_hour: 600,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceSettings {
    pub retention_days: u32,
    pub summary_mode: SummaryMode,
    #[serde(default = "default_summary_every")]
    pub summary_every: u32,
    pub rate_limits: RateLimits,
}
fn default_summary_every() -> u32 {
    3
}
impl Default for SpaceSettings {
    fn default() -> Self {
        Self {
            retention_days: 14,
            summary_mode: SummaryMode::Required,
            summary_every: default_summary_every(),
            rate_limits: RateLimits::default(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsChangeData {
    pub space_id: SpaceId,
    pub issuer_device: DeviceId,
    pub created_at: Timestamp,
    pub settings: SpaceSettings,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsChange {
    pub data: SettingsChangeData,
    pub signature: Signature,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Space {
    #[serde(default)]
    pub limits: ServerLimits,
    #[serde(default)]
    pub settings_change: Option<SettingsChange>,
    pub id: SpaceId,
    pub root_cert: DeviceCertificate,
    pub settings: SpaceSettings,
    pub created_at: Timestamp,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Person {
    pub id: PersonId,
    pub space_id: SpaceId,
    pub removed_at: Option<Timestamp>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub cert: DeviceCertificate,
    pub revoked_at: Option<Timestamp>,
    pub last_seen_at: Option<Timestamp>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grant {
    pub record: GrantRecord,
    pub sealed_keys: Vec<GrantSealedKey>,
    pub used_at: Option<Timestamp>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub membership: ChannelMembership,
    pub membership_history: Vec<ChannelMembership>,
    pub id: ChannelId,
    pub space_id: SpaceId,
    pub kind: ChannelKind,
    pub meta: Ciphertext,
    pub members: Vec<PersonId>,
    pub current_epoch: Epoch,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Blob {
    pub id: BlobId,
    pub space_id: SpaceId,
    pub size: u64,
    pub created_at: Timestamp,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadCursor {
    pub person_id: PersonId,
    pub thread_id: MessageId,
    pub last_read_seq: Seq,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Counts {
    pub unread_threads: u64,
    pub mentions: u64,
    pub claimed_by_you: u64,
    pub channels: Vec<ChannelCounts>,
    pub latest_seq: Seq,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelCounts {
    pub channel_id: ChannelId,
    pub unread_threads: u64,
    pub mentions: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum EventData {
    Message(Message),
    Reaction(Reaction),
    Revocation(Revocation),
    Settings(SettingsChange),
    ThreadState(ThreadState),
    ThreadDeleted(ThreadDeletion),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub seq: Seq,
    pub space_id: SpaceId,
    pub created_at: Timestamp,
    pub event: EventData,
}

/// The complete signed directory lets clients detect reused grants across devices.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrustBundle {
    pub root: DeviceCertificate,
    pub certificates: Vec<DeviceCertificate>,
    pub grants: Vec<GrantRecord>,
    pub revocations: Vec<Revocation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelMembershipData {
    /// Binds key bytes to the signed epoch, preventing unauthenticated box poisoning.
    pub key_commitment: PublicKey,
    /// The previous roster signature makes updates an authenticated chain.
    pub previous: Option<Signature>,
    pub space_id: SpaceId,
    pub channel_id: ChannelId,
    pub epoch: Epoch,
    pub kind: ChannelKind,
    pub members: Vec<PersonId>,
    pub issuer_device: DeviceId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelMembership {
    pub data: ChannelMembershipData,
    pub signature: Signature,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerLimits {
    pub max_attachment_bytes: u64,
    pub space_quota_bytes: u64,
}
impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            max_attachment_bytes: DEFAULT_MAX_ATTACHMENT_BYTES,
            space_quota_bytes: DEFAULT_SPACE_QUOTA_BYTES,
        }
    }
}
