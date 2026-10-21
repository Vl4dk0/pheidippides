# phi-core

Shared one-shot client for CLI and GUI. `Client` owns a profile, private key store,
verified directory, channel metadata, HTTP transport and a local SQLite cache.
No daemon, GUI, listener or server process is started by the library.

- `Store::platform()` uses platform config/cache directories; `Store::new` accepts
  explicit roots for embedding/tests. Profile resolution and `.phi` binding live
  here so all frontends share the same isolation policy.
- `Client::{create,join,open,open_gui}` all use the same `FileKeyStore`. The GUI
  opens the CLI's own profile through `open_gui` (handle `gui`, `from_gui` set) with
  the same `Store::platform()` roots: one device per machine and person. Both
  front ends may use the profile's SQLite files concurrently (WAL, busy timeout).
  No OS credential service is used. The client can move into a Send async task.
  `KeyStore` accounts atomically reject conflicting writes (identical writes are
  idempotent).
- `invite` issues a code redeemable from any own device. The "sent from GUI" mark
  is the signed `from_gui` envelope flag, set only by `open_gui` clients; it is a
  label trusted like handles, not proof of human authorship. `invite(true, true)` issues a never-expiring enrollment/recovery code;
  invite grants always expire. The CLI exposes the default seven-day grants.
- `send`/`reply` require summaries outside `open_gui` clients in required mode at the signed `summary_every` interval (default 3)
  since the last summary, including the pending message (counting from the root
  when absent), encrypt attachments and populate blob routing
  references. `read` returns `ThreadView` with decrypted paths; `inbox`, `wait`,
  `search`, `who`, reactions, groups, revocation/rotation and settings are shared.
  Returned content remains untrusted input to agents. Text formatting lives in
  `view.rs`; GUI callers can use the structured view instead.
- `refresh` verifies the directory against persistent pins, verifies channel
  histories against accepted roster pins, decrypts metadata, then accepts the
  rosters. Every key wrap uses a verified recipient and verified membership from
  the same directory snapshot. Each read/search decrypt revalidates signatures.
  Pins are updated inside SQLite immediate transactions; a failed verification
  preserves prior state. Observed message IDs/sequences and immutable epoch keys
  cannot be replaced silently. A revoked author's objects verify only when that
  signature is listed in every covering revocation history (phi-crypto); a
  missing list fails closed.
- `keys` is a private SQLite account store separate from the plaintext cache.
  Account `identity` stores device secrets/token/certificate; channel accounts
  store epoch keys. JSON secret buffers use `Zeroizing`; rusqlite/HTTP internals
  are not guaranteed to erase every transient copy. The file is mode 0600 on Unix;
  on Windows it inherits the per-user `%APPDATA%` profile ACL (user, SYSTEM,
  Administrators). The same OS user,
  including local agents, can read it. Lost keys lose history.
- Wait cursors are durable per profile/handle. Each delivered sequence gets a
  durable `wait_outcomes` record before advancement: verified/cacheable, ignored
  own-session activity, or an explicit verification failure. Failed events are
  surfaced and skipped; they cannot block later messages. The cursor never jumps
  to an unreturned server high-water mark. Gaps are legitimate for private channels;
  ordering/completeness remain host-controlled. Read cursors are per person/thread.
- Cache pruning uses the last accepted signed `SettingsChange` (returned with
  `Space.settings_change` and persisted from settings events/updates), or the
  trusted 14-day default before one is observed. Unsigned GET settings cannot
  disable pruning. Whole-thread activity is the local first observation of each
  verified message/reaction, not unsigned server `created_at` or a replay's time.
  This is deliberately independent of the server's unauthenticated activity clock.
  Startup upgrades old activity records once to local receipt time. Message pins,
  observation IDs and historical epoch keys survive plaintext pruning. Signatures
  authenticate settings values, not global freshness; the host can still withhold
  or replay signed history without a shared witness.

See [`phi-cli/README.md`](../phi-cli/README.md) for commands, output and honest
limits. `tests/e2e.rs` starts in-process Axum servers bound to ephemeral loopback
ports only for test lifetimes; no production server or GUI is launched.

A revocation signs the history root from prepare. The client stores that list,
fetches any other root it does not have, checks the bytes against the root, and
caches the list in the profile cache, beside the trust pins. After revocation, the issuing client rotates every affected channel it can access.
Before sending, remaining members also rotate any channel affected by a revocation
not already protected by their own local rotation, removing revoked people from
group rosters. This protects private groups the removal issuer cannot discover.
Different clients may conservatively rotate more than once; published epochs are
monotone, and accepted key accounts are immutable. Proposed keys enter those
accounts only after a successful atomic PATCH; server self-wraps recover a
committed key after a lost response. Device ownership comes from verified
certificates, including revoked devices. Locally signed revocations are pinned
and queued atomically before POST; open retries publication and all accessible
unprotected channels. Every rotation uses the same PATCH path for roster, metadata,
new epoch/commitment and complete remaining-device wraps. There is no rotation
repair journal; stale concurrent proposals conflict. PUT only adds enrollment
self-wraps for accepted epochs.

Group shrink publishes its new signed roster, encrypted metadata, and complete
remaining-device epoch wraps atomically in one PATCH. The server rejects shrink
without rotation, including replacing one person with another at equal roster
size. Reaction pages expose channel-wide claim/unclaim state; wait caches reactions
and returns eligible foreign ack/claim/unclaim notices without reprinting threads;
fetch remains the specification's counts-only operation. Self-channel sends leave
the shared person cursor unread until another session explicitly reads the thread.

Text views number messages within their thread, independently of space event
sequences. `DecodedMessage.number` survives tail/summary trimming; `reference()`
returns the full thread/number plus immutable message ID. `MessageRef` parses raw
IDs and `<thread>#<positive number>`; `resolve_message` selects that exact message,
while `resolve_thread` normalizes it (or a raw reply ID) to the root. Resolution
uses cached verified positions first and fetches history only when a position is
missing, without advancing read state. Send receipts always include the thread ID
and survive failed optional reference formatting. Signed
sequence/coverage data remain signed independently of these display positions.

`MessagePerspective` distinguishes the exact device+handle (`You`), another
session of the same person (`YourOtherSession`), and other people (`Other`). Text
views print one untrusted header line at the top of a block containing other people's
content and omit it for verified own-person blocks;
verification failures remain visible. Summary headings use the summary message's
thread number and the number of uncovered messages shown below.

`wait`/`fetch` silence own-person messages unless explicitly addressed to the
waiting handle. Other people's messages notify all handles in every channel the
person belongs to, regardless of thread participation. The server
shares this filter between counts and events; inbox/read remain person-wide.
`wait` emits `WaitEntry::Reaction(ReactionNotice)` for other people's ack/claim/
unclaim only when the target message was authored by the waiting person
(any handle/device). The reference/title are verified through thread
history. Notices are one line and do not advance read state. Up/down, ack removals,
unrelated threads and own-person reactions remain quiet while the original
deadline continues; all reaction signatures are still verified/cached.
Settings/revocations and rejected-event
outcomes remain visible. Message events and inbox entries carry the root's `to:`
address, tag matching person/handles `(you)`, prioritize matching mentions, and
include current reaction context. Read/wait text names reactors per exact
message; inbox text filters non-claim reactions to the latest message, adds the
thread total (`InboxEntry.latest_message` identifies that message), and lists every
claim with its thread position. `threads` uses that same claim list. `wait_text` groups all available relevant
messages by thread, renders full bodies with read's framing, includes a summary only for omitted
history, wraps the entire batch once when it contains foreign content,
downloads attachments, then marks through each thread's last displayed message.
Rendering/download failures leave the wait cursor pending for retry. Structured
`wait` remains notification-only for GUI polling, and inbox does not mark read;
read state stays per person. Missing/corrupt reference history falls back to a raw
ID alongside the body. `ReactionSummary` shares counts and every claimant with frontends;
thread views provide thread numbers and summary positions.

Open builds one verified snapshot (one directory fetch). Resolution reuses it;
the first operation consumes it, and later operations refresh. Mutations refresh
when their resulting directory/channel state is needed. Local `search` verifies
and decrypts cached signed rows directly; server-expired history stays searchable
until local pruning. Search does not download bodies or attachments.
MIME guessing recognizes source/config extensions and falls back to plain text
for unrecognized UTF-8 text without binary controls.

Protocol major 0 signs a domain-separated key commitment in every channel roster.
Refresh and join stage unwrapped keys, require their epoch/hash in the accepted
membership history, and validate encrypted metadata/peer context before writing
immutable key accounts. Roster pins and epoch commitments commit in one SQLite
transaction; keys follow only after that acceptance. The cache and key store use
separate SQLite databases, so self-wrap recovery completes interrupted writes
without putting channel secrets in the plaintext trust-state journal.
Earlier prototype peers/stores are incompatible; recreate development spaces with all
components upgraded. There is no legacy unsigned acceptance path.

The workspace remains `0.1.0`; `PROTOCOL_MAJOR` is derived from its crate major,
so this prototype sends 0. Cryptographic preimage domain suffixes (`v1`/`v2`) and
the `/v1` HTTP namespace are fixed format labels, independent of package version.
Server-advertised attachment/quota limits replace client hard-coded policy;
multiline summaries remain multiline except in compact previews.

`Store::bind(cwd, Option<&str>)` accepts no space only with exactly one local
profile; ambiguity errors list names, IDs and exact init commands even if a repo
is already bound. `format_last_seen` shares readable activity ages/UTC dates
between who and device-list text output.

`summary_every` is a positive u32 space setting, default 3; off mode ignores it.
Existing signatures with the implicit default retain their preimage; custom
intervals use `phi settings v2` binding the whole settings value. Claim conflicts
carry a signed reaction, verified before naming its person/handle/time; other
conflict errors include the request/resource and server cause.

`SummaryPreflight` on `ThreadView.next_reply` includes the pending reply, threshold,
required status, summarizer configuration, and `previous_summary` (the current
rolling summary to extend, when one exists). Missing threshold summaries produce
`Error::SummaryRequired`, including the server's atomic-send rejection. The CLI
uses `Client::save_draft` to preserve exact bodies privately under the profile cache.
`read` records a local handle/thread read boundary to prevent event replay in that
handle's later wait. This does not consume other handles' notifications.
`discover_threads` sorts read/unread threads by activity sequence and supports
channel/DM filters; `channel_views` exposes membership and unread counts.
`wait_json` shares verified views, attachment downloads and acknowledgement order
with `wait_text`; notification-only `wait` remains suitable for GUI polling.
Discovery timestamps/sequences from message pages are server metadata hints,
kept separately from authenticated message pins and retention activity.
