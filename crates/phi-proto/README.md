# Protocol major 1 contract

`Phi-Protocol: 1`, JSON, `/v1`. Request structs and query types implement
`Validate`; call it after serde parsing and before sending. Newtypes validate
on construction/deserialization. Responses accept unknown fields, including nested fields. Request wrappers use
`deny_unknown_fields`; their nested objects reject unknown fields using the same
typed schema. The signature
preimages and derivations are in `../phi-crypto/README.md`.

| Method/path | Request or query | Response |
|---|---|---|
| POST /spaces | CreateSpaceRequest | CreateSpaceResponse |
| GET /spaces/{sid} | — | GetSpaceResponse |
| PATCH /spaces/{sid}/settings | UpdateSettingsRequest | UpdateSettingsResponse |
| POST /spaces/{sid}/grants | CreateGrantRequest | CreateGrantResponse |
| GET /grants/{gid} | — | GetGrantResponse |
| POST /grants/{gid}/redeem | RedeemGrantRequest | RedeemGrantResponse |
| GET /spaces/{sid}/devices | — | ListDevicesResponse |
| POST /spaces/{sid}/revocations | CreateRevocationRequest | CreateRevocationResponse |
| POST /spaces/{sid}/channels | CreateChannelRequest | CreateChannelResponse |
| GET /spaces/{sid}/channels | — | ListChannelsResponse |
| PATCH /channels/{cid} | UpdateChannelRequest | UpdateChannelResponse |
| PUT /channels/{cid}/keys | PutKeysRequest | GetKeysResponse |
| GET /channels/{cid}/keys/mine | — | GetKeysResponse |
| POST /channels/{cid}/messages | SendMessageRequest | SendMessageResponse |
| GET /channels/{cid}/messages | MessagesQuery | ListMessagesResponse |
| POST /messages/{mid}/reactions | ReactionRequest (remove=false) | ReactionResponse |
| DELETE /messages/{mid}/reactions/{kind} | ReactionRequest (remove=true) | ReactionResponse |
| PUT /threads/{tid}/cursor | PutCursorRequest | PutCursorResponse |
| GET /spaces/{sid}/counts | CountsQuery | CountsResponse |
| GET /spaces/{sid}/events | EventsQuery | EventsResponse |
| POST /blobs | UploadBlobRequest | UploadBlobResponse |
| GET /blobs/{id} | — | DownloadBlobResponse |

Paths above are prefixed `/v1`. GET grant retrieval and PATCH channel membership
complete flows implicit in the spec's API sketch. Grant GET is public; it reveals
only signed public data and ciphertext to someone who knows the opaque grant ID.
The directory is one root plus all other certificates, grants and revocations;
do not repeat the root in certificates. Redemption returns it for trust validation.
Initial creation supplies root plus `all` and creator `self` channels. Grant joins
upload their own rewrapped keys via PUT keys; issuing an invite shares only #all.
Member self/DM channels are created using the standard channel endpoint.

Messages contain a nested `envelope` with the exact signed routing fields.
Servers derive author IDs from the authenticated device, assign seq/created_at,
and set summary_covers_seq to the assigned message seq iff has_summary=true.
The root uses thread_id=null; replies reference the root message ID. Empty
attachments/body are allowed. The message ciphertext limit includes the AEAD tag.
Encrypted channel metadata is a `Ciphertext` (nonce+ciphertext); the `meta_epoch` field records which epoch encrypted it, independently of
`current_epoch` used for new messages. Creation starts both at 0; key rotation
advances current_epoch and metadata updates supply their meta_epoch.

Limits fixed here: ciphertext 256 KiB decoded (including tag), handle 40 ASCII
bytes, title 1024 bytes, display name 255 bytes, mentions 128, attachments 32,
filename/MIME 255 bytes, members and per-request wrapped/sealed keys 1024,
message page 1..=1000, wait up to 900 seconds. Attachment size defaults to 25 MiB
and quota 2 GiB; both are server policy and configurable. Blob JSON contains
base64 ciphertext (plaintext file limit plus 16-byte tag); HTTP body-size limits
must allow base64 expansion and JSON overhead. Enforce request body limits before
JSON allocation. Grant default lifetime is 7 days. Retention/rate defaults match
SPEC.md. The title/summary/attachment-list limits are implementation choices
where the spec did not set one.

Validate path IDs with their newtypes, and compare all body IDs against path IDs,
authenticated device and database scope in the server. `Validate` checks local
shape; cryptographic proof, membership, claim exclusivity, quotas, revocation,
expiry, monotone epochs and cursor permissions require context in M2/M3.
Protocol mismatch returns exactly ErrorBody::Upgrade with HTTP 426. Use Retry-After
from the RateLimited error on 429. Names, titles, bodies and attachment descriptors
are encrypted. The host sees handles, mentions, reaction kinds, timestamps, sizes
and the membership/routing graph.
