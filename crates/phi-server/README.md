# phi-server

Ciphertext relay for protocol major 0. The server never receives decryption keys,
message plaintext, names, titles, or attachment filenames. Run one server process
per data directory. `Server::open(config)?.router()` exposes the same Axum app to
in-process clients/tests; it starts no listener or background task itself.

## Configuration and operation

```sh
phi-server --config /etc/phi/server.toml
PHI_SERVER_CONFIG=/etc/phi/server.toml phi-server
phi-server --config /etc/phi/server.toml backup /path/to/new-backup
```

The default config path is `server.toml`. See
[`deploy/server.example.toml`](../../deploy/server.example.toml). Unknown config
fields fail startup. Every field can be overridden using `PHI_SERVER_` plus its
uppercase name; use `__` for nested fields, e.g. `PHI_SERVER_DATA_DIR`,
`PHI_SERVER_TLS__MODE`, `PHI_SERVER_TLS__DOMAIN`,
`PHI_SERVER_LIMITS__MAX_BLOB_BYTES`, `PHI_SERVER_DEFAULTS__RETENTION_DAYS`.
A config file is required; overrides apply afterward.

| Field | Default / requirement |
|---|---|
| `data_dir` | `data` (SQLite `phi.db`, `blobs/`, ACME cache `acme/`) |
| `listen` | `127.0.0.1:8443` |
| `allow_space_creation` | `token`; alternative `anyone` |
| `admin_token` | required nonempty secret when creation is `token` |
| `tls.mode` | `off` (plain HTTP behind proxy or on private network); `acme` |
| `tls.domain`, `tls.email` | required in ACME mode |
| `limits.max_blob_bytes` | 26,214,400 plaintext bytes per attachment, plus 16-byte AEAD tag |
| `limits.space_quota_bytes` | 2,147,483,648 total stored ciphertext bytes per space |
| `limits.messages_per_min`, `limits.messages_per_hour` | 30 / 600 per device |
| `defaults.retention_days` | 14; 0 keeps history forever |
| `defaults.summary_mode` | `required`; alternative `off` |

ACME uses production Let's Encrypt and TLS-ALPN-01: public port **443** must
reach `listen` directly, preserving ALPN. Port 80 is unnecessary. Configure
`listen = "0.0.0.0:443"`, or forward public 443 to 8443. Certificates/accounts
are cached under `data_dir/acme`. TLS off does not add TLS headers or trust proxy
identity; authorization always uses bearer tokens. SIGINT/SIGTERM stop serving.

SQLite uses WAL and indexes space sequences, channel/thread sequences,
membership, normalized mentions, claims, and retention activity. SQLite work runs
on Tokio's blocking pool under a process-wide database lock. Counts use indexed
mention metadata; existing databases backfill it once at startup. Blob writes/fsync,
download reads, and retention file cleanup run outside that lock. Idle long-polls
hold no database lock/connection or blocking worker: they share one watch sender
per accessed, authenticated space. Request work is serialized, appropriate for
the specified low message throughput. Only committed event inserts notify waiters. Poll authentication never updates
`last_seen_at`; ordinary authenticated traffic updates it at most once per minute.
No per-waiter polling timer/query loop.

Hourly retention also runs at startup. A thread's server activity includes
messages and reaction changes. Sweeps cascade roots/replies, reactions, cursors,
events and blob references; unreferenced blobs older than the retention window
are collected, including abandoned uploads. Shared blobs survive while any
retained thread references them. Durable garbage records retry interrupted file
cleanup. Retention is best effort beyond the server; transcripts and downloaded
client copies may remain.

Backup requires a **new** destination directory. SQLite's online backup API
copies the database, then copies every referenced blob while a reserved SQLite
writer lock prevents a separately running server from changing the snapshot's
files. Reads and idle waits can continue; writes wait. Restore by using the backup
as `data_dir`. A failed backup may leave an incomplete destination; it is never
reported as successful. ACME cache is excluded and can be reissued. Device private
keys are client-owned and cannot be recovered from server backups.

## HTTP API

All paths below start with `/v1`. JSON request/response type names are exported by
`phi-proto`. Every request, including public grant operations, needs
`Phi-Protocol: 0`. Success is HTTP 200 and JSON. Requests reject unknown fields,
including nested signed records. Typed query/path fields and size bounds are
validated; ordinary JSON bodies are capped at 1 MiB before parsing, blob uploads
at a configured base64-aware bound. Authentication precedes body consumption.
Blob JSON buffers ciphertext in memory; a single process-wide upload permit bounds
aggregate upload buffers to one maximum-size body plus decoded ciphertext. Excess
uploads get 429/Retry-After before their bodies are polled. Files are synced outside
the database lock, then authorization and quota are rechecked at publication;
interrupted or rejected uploads clean up their unpublished files.

| Method/path | Body or query | Response |
|---|---|---|
| POST `/spaces` | `CreateSpaceRequest` | `CreateSpaceResponse` |
| GET `/spaces/{sid}` | — | `GetSpaceResponse` |
| PATCH `/spaces/{sid}/settings` | `UpdateSettingsRequest` | `UpdateSettingsResponse` |
| POST `/spaces/{sid}/grants` | `CreateGrantRequest` | `CreateGrantResponse` |
| GET `/grants/{grant_id}` | — | `GetGrantResponse` |
| POST `/grants/{grant_id}/redeem` | `RedeemGrantRequest` | `RedeemGrantResponse` |
| GET `/spaces/{sid}/devices` | — | `ListDevicesResponse` |
| POST `/spaces/{sid}/revocations` | `CreateRevocationRequest` | `CreateRevocationResponse` |
| POST `/spaces/{sid}/channels` | `CreateChannelRequest` | `CreateChannelResponse` |
| GET `/spaces/{sid}/channels` | — | `ListChannelsResponse` |
| PATCH `/channels/{cid}` | `UpdateChannelRequest` | `UpdateChannelResponse` |
| PUT `/channels/{cid}/keys` | `PutKeysRequest` | `GetKeysResponse` |
| GET `/channels/{cid}/keys/mine` | — | `GetKeysResponse` |
| POST `/channels/{cid}/messages` | `SendMessageRequest` | `SendMessageResponse` |
| GET `/channels/{cid}/messages` | `MessagesQuery` | `ListMessagesResponse` |
| GET `/messages/{mid}` | — | Message |
| POST `/messages/{mid}/reactions` | `ReactionRequest`, `remove=false` | `ReactionResponse` |
| DELETE `/messages/{mid}/reactions/{kind}` | `ReactionRequest`, `remove=true` | `ReactionResponse` |
| PUT `/threads/{tid}/cursor` | `PutCursorRequest` | `PutCursorResponse` |
| GET `/spaces/{sid}/counts` | `CountsQuery` | `CountsResponse` |
| GET `/spaces/{sid}/events` | `EventsQuery` | `EventsResponse` |
| POST `/blobs` | `UploadBlobRequest` | `UploadBlobResponse` |
| GET `/blobs/{id}` | — | `DownloadBlobResponse` |

`Authorization: Bearer <device_token>` is required except for space creation and
grant GET/redeem. Space creation uses the **admin token** in that same header
when configured `token`. Tokens are 32 random bytes, canonical base64url;
only BLAKE3 hashes are stored. Revocation immediately invalidates all targeted
device/person tokens, including active waits. Inaccessible scoped resources
return 404. Grant IDs are opaque 256-bit capabilities to signed public data and
ciphertext, never the invite secret S. No endpoint lists spaces across tokens.

Initial creation requires a valid self-signed root and creator-only all/self
channels. Invites add their new person to #all; enrollment adds another device
to the issuer's person. Clients create additional self/DM/group channels, and
redeemers PUT their rewrapped keys. All membership is person-based. Group PATCH
may change membership/meta; all, self, and DM membership is fixed, with
metadata-only PATCH updates permitted for encrypted client-side name registries.
Removing any person (including an equal-size replacement) requires one atomic
PATCH with the next signed membership epoch, metadata encrypted at that epoch,
and `wrapped_keys` for every remaining active device. A rejected PATCH leaves the
old roster, epoch, and wraps unchanged. All rotations advance exactly one epoch in an atomic PATCH containing the
signed roster/commitment, metadata and complete remaining-device wraps. PUT only
adds the authenticated device's own same/historical-epoch enrollment wrap.

Messages must use the current epoch and a root thread in the same channel.
In required mode, summaries are enforced via the plaintext flag for CLI devices
only at three messages since the last summary, including the pending message.
With no summary, count from the root; the summary-bearing message resets the
count. The server checks inside the send transaction and explains the current
count/threshold in rejection errors. GUI devices are exempt. The host cannot verify summary contents. Sending advances
the author's person cursor except in self channels, so other sessions see those
messages as unread; PUT cursor explicitly sets it, including backwards
for unread, but cannot exceed the newest thread message. Counts return unread
threads, unread matching mentions (general mentions always count; no handle means
all handles), and all claims by the person. Claims are unique per message;
the same person can remove one from any handle, with a signed removal request.

Message pages are ascending by seq, default 100, maximum 1000. `next_after` is
the final returned message seq. Events default to a 600-second wait, capped at
900; `timeout=0` reads backlog immediately. Events include all messages in joined
channels, all reactions in those channels, and space settings/revocations.
Reaction author targeting is retained as routing metadata, not an access filter.
The optional handle is validated; highlighting targeted mentions is a client
responsibility, since every channel message is relevant. Events return at most
1000 rows. `next_after` is the last returned seq, or the requested `after` if
empty; it never advances through unreturned events. Private-channel gaps are normal. Clients keep separate cursors per profile/handle.
Concurrent waiters all observe events; notification registration precedes reads.

**Attachment senders must fill `SendMessageRequest.blob_ids`** with the uploaded
IDs embedded in encrypted content. It defaults to `[]` for wire compatibility,
is limited to 32 distinct IDs, and exists solely for access control and thread
retention. IDs are unsigned routing metadata; clients verify encrypted attachment
descriptors. Blob downloads require uploader ownership or membership in a thread's
channel referencing the blob. Sharing a blob requires the same access. Per-file
limits include 16 extra tag bytes; quotas count complete stored ciphertext.

Two persistent token buckets gate authenticated send attempts before body parsing
and signature verification (minute and hour); invalid requests also spend tokens; effective rates are the smaller of server limits and signed space
settings. Restarting does not reset buckets. HTTP 429 includes `Retry-After`
in seconds. Other operations do not consume message tokens.

## Verification and limits

The server verifies root hash/self-signature; issuer-signed grant records;
complete certificate/grant/revocation chains at redemption; current issuer status,
expiry and atomic single use; signed revocations/settings/reactions and their
authenticated identities; and message envelope signatures. Grant keys may cover
only the issuer's permitted channels (#all for invites; all owned channels for
enrollment), with current keys required. Revocation targets must exist in the
same space. Clients remain the real verifiers: verify the complete directory,
message signatures, revocations, and encrypted content independently. The host
cannot detect rolled-back/omitted records or truthful summaries.

The server persists opaque cryptographically verified trust pins in SQLite and
updates them in the same transaction as enrollment/revocation. Verification uses
those prior pins, so rapid same-second enrollment/removal works without trusting
backdated issuance. Invite person IDs are reserved uniquely, including unused
invites. Enrollment codes issued by any own device can enroll any kind of client.
The redeeming client declares its kind in its signed certificate; the GUI label
is trusted like handles and imposes no enrollment authorization policy.

Channel create/update/rotation verify signed membership scope, issuer, epoch and
member set. Updates/rotations extend the stored roster signature and preserve its
history; stale/forked updates return 409. #all additions are backed by signed invite
origins. Client-chosen message IDs and summary coverage are now in the signed
envelope. The server requires projected fields to match, rejects coverage beyond
existing thread history, and copies coverage without replacing it with the new
server seq. Clients must verify/persist both directory and channel pins as described
in phi-crypto/README.md; the host is not the client's source of trusted pins.

The old prototype's signed JSON lacks these proofs and cannot be silently migrated
by the server. Development data needs legitimate re-signing or recreation; no
legacy/unsigned verification path is provided.

Known metadata leak: handles, mentions, reaction kinds, timestamps, ciphertext
sizes, membership/routing graph, attachment IDs/thread associations, device kinds
and public keys. Already read content remains compromised after revocation.
Clients must rotate affected channels after removing a person/device; the server
cannot generate their encryption keys.

Errors use `phi_proto::ErrorBody` (no plaintext or bearer tokens in logs):

| HTTP | `error` | Meaning |
|---|---|---|
| 400 | `invalid` | precise schema, format, signature, or contextual validation failure |
| 401 | `unauthorized` | missing, unknown, or revoked device token |
| 403 | `forbidden` | admin token, grant expiry, or ownership restriction |
| 404 | `not_found` | nonexistent or inaccessible scoped resource |
| 409 | `conflict` | duplicate ID/reaction, used grant, existing claim/wrap, invalid epoch progression |
| 413 | `too_large` | HTTP body, per-file limit, or space quota |
| 426 | `upgrade` | missing/invalid/mismatching major; includes `required_major` |
| 429 | `rate_limited` | token bucket empty; includes `retry_after` and `Retry-After` |
| 500 | `internal` | generic storage failure; details stay in structured server logs |

Message pages also return the current signed reactions for each returned message
and the authenticated person's read cursors for the returned threads, allowing
client inbox/read composition without decrypting anything on the host. Direct
GET message lookup has the same space/channel authorization as message pages.

Protocol major 0 binds channel key commitments into signed membership proofs.
Use upgraded clients and recreate earlier development spaces; signatures cannot
be migrated by the host. Routes retain `/v1`. Space GET includes the last signed
`settings_change`, persisted atomically with the settings update/event. Exact
revocation re-publication is idempotent, allowing recovery after a lost response.
Server-configured defaults govern server cleanup. Clients use their trusted 14-day
baseline until an authorized signed settings update is observed.

Routes are registered with typed Axum path/query/body extractors. Shared protocol
and auth middleware precede body reads; message-rate and upload-semaphore admission
remain pre-body, with authorization checked again in the commit DB task. Event
notifications happen only after an event commit, not uploads/cursors/counts.
`Space.limits` always reflects current configured attachment/quota limits. Default
25 MiB uploads need about 33.4 MiB base64; proxy examples allow 40 MiB and must be
raised when changing server limits. Version 0.1.0 derives protocol major 0 directly
from Cargo's package major; `/v1` remains the route namespace.
