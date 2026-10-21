# Cryptographic architecture (protocol major 0)

This crate is pure: no files, network, clocks, or OS randomness. Pass an `Entropy`
implementation backed by the OS CSPRNG from the application, and Unix seconds to
trust verification. Deterministic test entropy must never be used in an app.
`DeviceKeys::export_secrets` and `ChannelKey::export_secret` return `Zeroizing`
buffers for protected storage. Secret owners, decoded key buffers, decrypted
bytes, temporary plaintext JSON and encoded links zeroize on drop; key-bearing
Debug output is redacted. Caller-owned input/output strings and copies remain
the caller's responsibility. Third-party hash/KDF internals are not guaranteed
to erase all intermediate state.

## Bytes and keys

All wire binary values are canonical RFC 4648 URL-safe base64 without padding.
Ordinary IDs are 16 bytes (22 characters). Space IDs are the first **16 raw bytes**
of BLAKE3(root Ed25519 public key), then base64url encoded; grant IDs use 32 bytes
(43 characters). Public keys and symmetric keys are 32 bytes, Ed25519 signatures
64 bytes, XChaCha20 nonces 24 bytes. Timestamps are unsigned Unix seconds, epochs
and sequence numbers unsigned 64-bit integers. Keys are per space, per device.

Ed25519 signing keys use random 32-byte seeds; X25519 static secrets use independent
random 32-byte input. Verification uses dalek's strict verification and rejects
weak signing public keys. Channel epoch 0 starts with a random 32-byte key;
rotation generates an independent key and checked-increments the epoch. Keep old
`ChannelKey`s in the client store for history. Epoch overflow is an error.

For random 32-byte grant secret S:

- `grant_id = blake3::derive_key("phi grant id", S)` (all 32 bytes).
- Ed25519 seed = HKDF-SHA256, IKM=S, salt absent (32 zero bytes),
  info=UTF-8 `phi grant sign`, L=32.
- Grant symmetric sealing key = same HKDF, info=UTF-8 `phi grant seal`, L=32.

Grant channel keys are sealed symmetrically rather than via an asymmetric box:
the only recipient is whoever possesses S. S never appears in a wire request.

## Canonical signature and AAD bytes

`J(domain, fields)` means UTF-8 `serde_json::to_vec(&(domain, fields))`, with no
whitespace. `fields` is an **ordered JSON array**, not an object. IDs and binary
values are base64 strings; enums are their snake_case wire strings; absent values
are JSON `null`; integer formatting is decimal. This avoids JSON object-order
ambiguity. Mention lists preserve order and encode each mention as
`[person_id, handle_or_null]`. No map enters a signed preimage.

| Domain | Ordered fields |
|---|---|
| `phi message v2` | id, summary_covers_seq, space_id, channel_id, thread_id, epoch, handle, from_gui, mentions, nonce, ciphertext |
| `phi device certificate v2` | space_id, person_id, device_id, sign_pub, kex_pub, created_at, issuer |
| `phi grant record v1` | space_id, grant_id, grant_pub, kind, person_id, created_at, expires_at, issuer_device |
| `phi revocation v2` | space_id, target, issuer_device, created_at, history |
| `phi reaction v1` | space_id, message_id, person_id, device_id, handle, kind, created_at, remove |
| `phi settings v1` | space_id, issuer_device, created_at, retention_days, summary_mode, messages_per_minute, messages_per_hour |
| `phi settings v2` | space_id, issuer_device, created_at, settings |
| `phi channel membership v2` | previous, key_commitment, space_id, channel_id, epoch, kind, members, issuer_device |
| `phi thread state v1` | space_id, thread_id, person_id, device_id, handle, closed, created_at |
| `phi thread delete v1` | space_id, thread_id, person_id, device_id, handle, created_at |

A certificate issuer is `['device', device_id]` or `['grant', grant_id]`; a
revocation target is `['device', device_id]` or `['person', person_id]` (JSON uses
double quotes). `history` is the base64url history root. `phi settings v2` is
used when `summary_every` is not the default; otherwise the change signs
`phi settings v1` and does not bind `summary_every`. The wire issuer references the grant ID; its signed grant record
supplies the grant public key. The root certificate names itself as device issuer
and is self-signed. Only the root may use a device issuer: every other certificate
must be grant-signed. Client-chosen message IDs and coverage claims are signed;
server-assigned seq/created_at and author projections are not. Call
`VerifiedDirectory::verify_message` before decrypting: it checks author identity,
ID equality and both summary projections against the signed envelope.

`summary_covers_seq` is a signed optional upper bound on **already assigned thread
sequences**, plus the current message identified by its signed ID. `None` means no
summary; `Some(0)` is the only root summary claim. Reply senders supply the latest
thread seq their summary actually covers. The server requires coverage <= existing
thread history and copies the claim unchanged into Message; it never substitutes
its newly assigned seq. Decryption also checks summary presence against the claim.
Readers must use this authenticated coverage, never infer coverage from the
message's untrusted server seq. Deduplicate IDs and retain known ID/seq associations
in the client cache. This does not make server ordering, completeness, counts or
summary truth cryptographically trustworthy.

Encryption is XChaCha20-Poly1305 with fresh random nonce; ciphertext includes the
16-byte tag. Message plaintext is `MessageContent` JSON. Message AAD is
`J('phi message aad v1', [space_id, channel_id, thread_id, epoch, handle, mentions])`.
Metadata AAD is `J('phi channel metadata v1', [space_id, channel_id, epoch])`.
Attachment AAD is the literal UTF-8 bytes `phi attachment v1`; each file has its
own random key and nonce carried inside the encrypted message. Never execute
received attachments; filenames are validated but clients must still contain
cache writes within their cache directory.

## Sealed boxes and key distribution

Generate a fresh ephemeral X25519 static secret per box; reject non-contributory
(all-zero) DH shared results. Derive a 32-byte key with HKDF-SHA256, IKM=DH shared
secret, absent salt, info=`UTF8('phi sealed box v1') || ephemeral_pub_raw ||
recipient_pub_raw`. Encrypt with fresh random XChaCha20 nonce and caller-supplied
AAD. The wire box contains ephemeral_pub, nonce and ciphertext.

Channel wrapping requires both a `VerifiedDevice` and `VerifiedMembership` from
the same verified directory snapshot. It checks space, channel, epoch and person
membership and the signed key commitment before sealing, so a valid space
certificate alone is insufficient. `ChannelKey::commitment()` is the 32-byte
BLAKE3 derive-key hash with context `phi channel key commitment v2` over the fixed
raw bytes `space_id || channel_id || epoch_u64_le || key`. It is public evidence,
not a key. Before storing an unwrapped/grant key, clients must match this digest
against the pinned, verified epoch history; a sealed box alone authenticates no
sender. A former key holder therefore cannot poison a new epoch with old bytes,
even by also forging valid metadata under those bytes.
AAD is
`J('phi wrapped key v1', [space_id, channel_id, epoch, device_id])`. Grant-sealed
channel key AAD is `J('phi grant channel key v1', [space_id, grant_id, channel_id,
epoch])`. Both carry exactly 32 plaintext bytes (48 ciphertext bytes).

## Trust, pinned state and M3 persistence contract

Call `verify_trust_bundle(expected_space_id, bundle, now, prior)` with
`Option<&PinnedTrustState>`. `None` is **first-use bootstrap only**. On success:

1. Inspect `grant_certificates()` (grant ID -> exact certificate) and
   `revocations()`; these are verified evidence, not server assertions.
2. Atomically persist `pinned_state()` in this profile's locally trusted cache.
   `PinnedTrustState` supports serde for that cache. Never deserialize host data
   as pins, pass a current response as its own prior, or discard existing pins
   after an error. M3 owns filesystem persistence and profile isolation.
3. Pass those pins on every refresh. Missing or changed pinned root, certificates
   or grant records fail closed. The exact certificate pins also prevent swapping
   a grant's redeemer for another device across successive snapshots.
4. Pinned revocations are always merged, even when the host omits them. Save the
   resulting merged state; never replace it with a raw response. A missing
   revocation therefore cannot make its target active again.

The root hash/self-signature anchors all dependencies. Verification rejects
cycles, mixed spaces, duplicate devices/grants, reused grants and invalid
signatures. Each person has exactly one origin: the root person or one invite
record; another invite cannot reuse that person ID, even if the first invite is
unused. Later certificates for that person must use enroll grants issued by that
same person's device. Device certificates carry no client kind. The GUI mark is
the signed message envelope's `from_gui` flag: a client-declared label trusted like
handles, not hardware/keychain attestation or proof that a human typed the
message. Dropping the kind field and adding the flag changes certificate and
message signing bytes; earlier prototype spaces must be recreated.

A revocation signs a history root (`phi revocation v2`). After that revocation is
known, an object signed by a covered key verifies only when its exact signature
is in the list of every revocation covering the key. A missing list is an empty
set, so the object fails closed. The one exception is a roster record that a
later active signer's record extends: the `previous` signature inside the
membership preimage endorses it. Revocation is monotone. A valid signature takes
effect, and no later object, revocation, pin or cache makes a revoked device
active again. There is no certificate pin exception and no wall-clock ordering
of who revoked whom. `authorize` returns `Ok(true)` for an active device,
`Ok(false)` when the device is revoked and the signature is listed, and
`Err(Revoked)` otherwise. `VerifiedDevice::active()` is that bit: false means
listed evidence from a revoked device, and `wrap_channel_key` refuses a
recipient that is not active. Reactions are not listed. `reaction_counts`
returns `Ok(false)` when the device is revoked or its issuer chain is broken; a
bad signature is still an error. `history_root` rejects a list that is not
strictly ascending and hashes the raw signature bytes with BLAKE3 derive-key
`phi revocation history v1`. The empty list has one fixed root.

Timestamps still check creation bounds and expiry, but are not an ordering oracle.
Expiry is exclusive at redemption; an old redeemed certificate remains valid after expiry.
Only enroll/recovery grants may have `expires_at=null`.

A fresh client fetches each unknown history once, checks the bytes against the
root, and caches the list. It does not guess which descendants predate a
revocation. Pins detect conflicts against *observed* history. No protocol without a shared
witness can discover a revocation nobody has seen or reconcile isolated clients'
divergent first snapshots automatically. Cross-client exchange of trusted pins
would expose equivocation; this prototype does not add a transparency service.

## Authenticated channel membership

Create/update carry `ChannelMembership`; every rotation is an atomic channel
PATCH including new-epoch metadata and complete wraps. PUT keys is enrollment
self-rewrap only.
Each proof signs space/channel/epoch/key commitment/kind/members/issuer and the previous roster's
signature (`previous=null` for epoch-0 genesis). Updates and rotations must extend
the currently stored proof; the server persists the complete oldest-first
`Channel.membership_history` plus the current `membership`. Historical/self key rewraps leave membership unchanged.

Before wrapping, call `VerifiedDirectory::verify_channel(channel, prior_roster)`
and pass its `VerifiedMembership` to `wrap_channel_key`. M3 must persist each
accepted current roster per channel, pass it on every refresh, and retain it on
errors. The verifier checks history back to that exact pin, immutable space/id/kind,
nondecreasing epoch, immutable commitment within each epoch, signatures, and that each new signer belonged to the prior
roster. It rejects a missing pin, fork, rollback or unsigned members projection.
For locally proposed creation/rotation, `verify_membership(proposed, prior_roster)`
checks the one-step transition before wrapping. `None` is only for a newly accepted
epoch-0 genesis; first discovery must also validate the channel's decrypted
metadata/expected peer context. A first-ever roster remains a bootstrap trust
observation, not proof of unobserved history.

`#all` has a root-signed genesis and is special by definition: every person with
a verified certificate belongs to it. Signed invite origins authorize automatic additions, so effective all
membership comes from the verified directory rather than an unsigned host list;
new roster signatures checkpoint that set. Removed/revoked devices remain excluded
by the directory. Group, DM and self recipients are restricted to their signed
member sets. Refresh directory and roster together before rotating; stale verified
objects from a different directory snapshot cannot be mixed.

## Text and links

Titles, display names, attachment names and MIME values reject control
characters, Unicode line/paragraph separators and bidi directional controls. Summaries allow bounded multiline text while
rejecting bidi/non-text controls.
Attachment names remain one path segment. Markdown bodies remain multiline and
untrusted: M3 must render them inside the untrusted content frame, never as trusted
header/author markup. The `from_gui` flag never grants instruction priority.

Links are `phi://authority/space_id#S` (HTTP) or `phis://...` (HTTPS), using the
32-byte S encoded base64url. DNS, IPv4 and bracketed IPv6 with optional valid port
are accepted; credentials, paths, queries, percent escapes and padding are rejected.
Encoded links use `Zeroizing<String>` and expose S only via explicit accessors.

The signed commitment changes the unreleased prototype to protocol major 0.
Earlier prototype peers are rejected at the protocol header. Old membership records lack the
mandatory field/signature domain; recreate development spaces with upgraded
components. The host cannot migrate signatures and no legacy verification exists.
`VerifiedDirectory::certificate(id)` exposes authenticated ownership even after
revocation; it grants no active authorization. `VerifiedDevice::active()` is the
authorization bit for wrapping and for treating the signer as current.
`trust_bundle()` exports verified
merged evidence for locally pinning a newly signed revocation before publication.

The protocol header follows workspace Semver major (0 for 0.1.0), independently
of the fixed signature/AAD format-domain suffixes listed above.
