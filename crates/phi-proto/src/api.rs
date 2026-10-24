use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSpaceRequest {
    #[serde(deserialize_with = "crate::validation::strict")]
    pub root_cert: DeviceCertificate,
    #[serde(deserialize_with = "crate::validation::strict")]
    pub all_channel: CreateChannelRequest,
    #[serde(deserialize_with = "crate::validation::strict")]
    pub self_channel: CreateChannelRequest,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateSpaceResponse {
    pub space: Space,
    pub device_token: Base64<32>,
}
pub type GetSpaceResponse = Space;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSettingsRequest {
    #[serde(deserialize_with = "crate::validation::strict")]
    pub change: SettingsChange,
}
pub type UpdateSettingsResponse = SettingsChange;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGrantRequest {
    #[serde(deserialize_with = "crate::validation::strict")]
    pub record: GrantRecord,
    #[serde(deserialize_with = "crate::validation::strict")]
    pub sealed_keys: Vec<GrantSealedKey>,
}
pub type CreateGrantResponse = Grant;
/// GET /v1/grants/{grant_id} is unauthenticated: only signed records and ciphertext.
/// Redeemers need this before signing their certificate; it never exposes S.
pub type GetGrantResponse = Grant;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedeemGrantRequest {
    #[serde(deserialize_with = "crate::validation::strict")]
    pub certificate: DeviceCertificate,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RedeemGrantResponse {
    pub device_token: Base64<32>,
    pub grant: Grant,
    pub directory: TrustBundle,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceHold {
    pub device_id: DeviceId,
    pub holder_device: DeviceId,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListDevicesResponse {
    pub persons: Vec<Person>,
    pub devices: Vec<Device>,
    pub directory: TrustBundle,
    pub holds: Vec<DeviceHold>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareRevocationRequest {
    pub target: RevocationTarget,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryResponse {
    pub signatures: Vec<Signature>,
}
pub type PrepareRevocationResponse = HistoryResponse;
pub type GetHistoryResponse = HistoryResponse;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRevocationRequest {
    #[serde(deserialize_with = "crate::validation::strict")]
    pub revocation: Revocation,
}
pub type CreateRevocationResponse = Revocation;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateChannelRequest {
    #[serde(deserialize_with = "crate::validation::strict")]
    pub membership: ChannelMembership,
    pub id: ChannelId,
    pub space_id: SpaceId,
    pub kind: ChannelKind,
    #[serde(deserialize_with = "crate::validation::strict")]
    pub meta: Ciphertext,
    pub members: Vec<PersonId>,
    #[serde(deserialize_with = "crate::validation::strict")]
    pub wrapped_keys: Vec<WrappedKey>,
}
pub type CreateChannelResponse = Channel;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListChannelsResponse {
    pub channels: Vec<Channel>,
}
/// One publication keeps roster, metadata and key recipients consistent after any rotation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateChannelRequest {
    #[serde(default, deserialize_with = "crate::validation::strict")]
    pub wrapped_keys: Vec<WrappedKey>,
    #[serde(deserialize_with = "crate::validation::strict")]
    pub membership: ChannelMembership,
    pub epoch: Epoch,
    #[serde(deserialize_with = "crate::validation::strict")]
    pub meta: Ciphertext,
    pub members: Vec<PersonId>,
}
pub type UpdateChannelResponse = Channel;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutKeysRequest {
    pub epoch: Epoch,
    #[serde(deserialize_with = "crate::validation::strict")]
    pub wrapped_keys: Vec<WrappedKey>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetKeysResponse {
    pub wrapped_keys: Vec<WrappedKey>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageRequest {
    pub id: MessageId,
    #[serde(deserialize_with = "crate::validation::strict")]
    pub envelope: MessageEnvelope,
    pub signature: Signature,
    pub has_summary: bool,
    /// Routing references let retention collect attachments without decrypting content.
    #[serde(default)]
    pub blob_ids: Vec<BlobId>,
}
pub type SendMessageResponse = Message;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessagesQuery {
    pub thread: Option<MessageId>,
    pub after: Option<Seq>,
    pub limit: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListMessagesResponse {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub cursors: Vec<ReadCursor>,
    #[serde(default)]
    pub activities: Vec<ThreadActivity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<ThreadDeletion>,
    pub next_after: Option<Seq>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReactionRequest {
    #[serde(deserialize_with = "crate::validation::strict")]
    pub reaction: Reaction,
}
pub type ReactionResponse = Reaction;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadStateRequest {
    #[serde(deserialize_with = "crate::validation::strict")]
    pub state: ThreadState,
}
pub type ThreadStateResponse = ThreadState;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteThreadRequest {
    #[serde(deserialize_with = "crate::validation::strict")]
    pub deletion: ThreadDeletion,
}
pub type DeleteThreadResponse = ThreadDeletion;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutCursorRequest {
    pub last_read_seq: Seq,
}
pub type PutCursorResponse = ReadCursor;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountsQuery {
    pub handle: Option<Handle>,
}
pub type CountsResponse = Counts;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsQuery {
    pub after: Seq,
    pub timeout: Option<u16>,
    pub handle: Option<Handle>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventsResponse {
    pub events: Vec<Event>,
    pub next_after: Seq,
}
/// JSON blob transport keeps the prototype API uniform. Servers may stream decoding.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadBlobRequest {
    pub space_id: SpaceId,
    pub ciphertext: Base64Bytes,
}
pub type UploadBlobResponse = Blob;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadBlobResponse {
    pub blob: Blob,
    pub ciphertext: Base64Bytes,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum ErrorBody {
    Invalid {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary_required: Option<SummaryRequirement>,
    },
    Unauthorized {
        message: String,
    },
    Forbidden {
        message: String,
    },
    NotFound {
        message: String,
    },
    Conflict {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claim: Option<Box<Reaction>>,
    },
    TooLarge {
        message: String,
    },
    RateLimited {
        retry_after: u64,
    },
    /// Hosts older than the Phi-Protocol header omit `server`.
    Upgrade {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server: Option<String>,
    },
    Internal {
        message: String,
    },
}
impl ErrorBody {
    pub fn status_code(&self) -> u16 {
        match self {
            Self::Invalid { .. } => 400,
            Self::Unauthorized { .. } => 401,
            Self::Forbidden { .. } => 403,
            Self::NotFound { .. } => 404,
            Self::Conflict { .. } => 409,
            Self::TooLarge { .. } => 413,
            Self::RateLimited { .. } => 429,
            Self::Upgrade { .. } => 426,
            Self::Internal { .. } => 500,
        }
    }
}
pub fn check_protocol(version: &str) -> Result<(), ErrorBody> {
    if version == PROTOCOL {
        Ok(())
    } else {
        Err(ErrorBody::Upgrade {
            server: Some(PROTOCOL.into()),
        })
    }
}

/// Discovery hints never participate in authenticated content or retention decisions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadActivity {
    pub thread_id: MessageId,
    pub last_activity: u64,
    pub last_activity_seq: Seq,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<PersonId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ThreadState>,
}

/// An additive error detail lets clients recover drafts without parsing human prose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryRequirement {
    pub bytes_since_summary: u64,
    pub threshold: u32,
}
