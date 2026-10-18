# Cryptographic architecture (protocol major 1)

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
| `phi message v1` | space_id, channel_id, thread_id, epoch, handle, mentions, nonce, ciphertext |
| `phi device certificate v1` | space_id, person_id, device_id, kind, sign_pub, kex_pub, created_at, issuer |
| `phi grant record v1` | space_id, grant_id, grant_pub, kind, person_id, created_at, expires_at, issuer_device |
| `phi revocation v1` | space_id, target, issuer_device, created_at |
| `phi reaction v1` | space_id, message_id, person_id, device_id, handle, kind, created_at, remove |
| `phi settings v1` | space_id, issuer_device, created_at, retention_days, summary_mode, messages_per_minute, messages_per_hour |

A certificate issuer is `['device', device_id]` or `['grant', grant_id]`; a
revocation target is `['device', device_id]` or `['person', person_id]` (JSON uses
double quotes). The wire issuer references the grant ID; its signed grant record
supplies the grant public key. The root certificate names itself as device issuer
and is self-signed. Signatures cover only the fields above, not storage IDs, seq,
author fields, summary flags or server timestamps. Check claimed author IDs against
the verified certificate with `VerifiedDirectory::verify_message` before decrypting.

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

Channel wrapping requires a `VerifiedDevice`, not an unverified public key; AAD is
`J('phi wrapped key v1', [space_id, channel_id, epoch, device_id])`. Grant-sealed
channel key AAD is `J('phi grant channel key v1', [space_id, grant_id, channel_id,
epoch])`. Both carry exactly 32 plaintext bytes (48 ciphertext bytes).

## Trust and links

`verify_trust_bundle(expected_space_id, bundle, now)` anchors the root hash and
self-signature, verifies all device/grant dependencies iteratively, and rejects
cycles, unknown issuers, duplicate IDs, multiple certificates per grant, mixed
spaces, invalid signatures, mismatched grant persons and redemption outside the
grant's validity interval. `created_at` was added to grant records to authenticate
issuance order. Enroll grants belong to the issuer's person; invite grants name a
new person. `expires_at=null` is allowed only for enroll/recovery. Expired grants
remain valid evidence for certificates issued before expiry.

Signed revocations cover a device or a whole person. Issuers must have been valid
at issuance, and revoked targets never become `VerifiedDevice`s. Earlier valid
issuance survives a later issuer revocation: revoking a root machine does not
revoke everyone's devices. Grants cannot be redeemed after their issuer was
revoked. This uses signed timestamps; the server must enforce current revocation,
single use and expiry at redemption. A dishonest host can omit/roll back directory
records or backdate server metadata; this prototype has no transparency log or
external freshness witness. Preserve known revocations in the client cache.

Links are `phi://authority/space_id#S` (HTTP) or `phis://...` (HTTPS), using the
32-byte S encoded base64url. DNS, IPv4 and bracketed IPv6 with optional valid port
are accepted; credentials, paths, queries, percent escapes and padding are rejected.
Encoded links use `Zeroizing<String>` and expose S only via explicit accessors.
