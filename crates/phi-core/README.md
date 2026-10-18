# phi-core

Shared one-shot client for CLI and GUI. `Client` owns a profile, private key store,
verified directory, channel metadata, HTTP transport and a local SQLite cache.
No daemon, GUI, listener or server process is started by the library.

- `Store::platform()` uses platform config/cache directories; `Store::new` accepts
  explicit roots for embedding/tests. Profile resolution and `.phi` binding live
  here so all frontends share the same isolation policy.
- `Client::{create,join,open}` provision CLI identities with `FileKeyStore`.
  `create_for_device`/`join_for_device` take `DeviceOptions`; GUI callers supply
  `kind=Gui` and a `KeyStoreFactory` which uses the generated profile's IDs to
  namespace keychain accounts. Use separate GUI config/cache roots via `Store::new`
  so CLI and GUI retain distinct credentials for the same server/space. `open_with_keystore` reopens that identity. GUI
  provisioning requires a supplied store and never creates a file key store.
  `KeyStore` receives secret bytes; callers must protect them and atomically reject
  conflicting writes to an existing account (identical writes are idempotent). GUI keychain
  implementation belongs to M5. The client can move into a Send async task.
- `invite_for_device` controls the signed target kind. GUI enrollment requires a
  GUI issuer. `invite(true, true)` issues a never-expiring enrollment/recovery code;
  invite grants always expire. The CLI exposes the default seven-day grants.
- `send`/`reply` enforce summaries, encrypt attachments and populate blob routing
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
  cannot be replaced silently. Revoked authors fail closed per phi-crypto.
- `keys` is a private SQLite account store separate from the plaintext cache.
  Account `identity` stores device secrets/token/certificate; channel accounts
  store epoch keys. JSON secret buffers use `Zeroizing`; rusqlite/HTTP internals
  are not guaranteed to erase every transient copy. Lost keys lose history.
- Wait cursors are durable per profile/handle and advance only after successful
  verification. Read cursors are server-side per person/thread. Cache retention
  follows observed whole-thread activity, preserving historical epoch keys.

See [`phi-cli/README.md`](../phi-cli/README.md) for commands, output and honest
limits. `tests/e2e.rs` starts in-process Axum servers bound to ephemeral loopback
ports only for test lifetimes; no production server or GUI is launched.

After revocation, the issuing client rotates every affected channel it can access.
Before sending, remaining members also rotate any channel affected by a revocation
not already protected by their own local rotation, removing revoked people from
group rosters. This protects private groups the removal issuer cannot discover.
Different clients may conservatively rotate more than once; published epochs are
monotone, and accepted key accounts are immutable. Proposed keys enter those
accounts only after a successful PUT; server self-wraps recover committed keys
after an interrupted response. Concurrent stale roster updates return conflict.

Group shrink publishes its new signed roster, encrypted metadata, and complete
remaining-device epoch wraps atomically in one PATCH. The server rejects shrink
without rotation, including replacing one person with another at equal roster
size. Reaction pages and wait events expose channel-wide claim/unclaim state;
fetch remains the specification's counts-only operation. Self-channel sends leave
the shared person cursor unread until another session explicitly reads the thread.

Text views number messages within their thread, independently of space event
sequences. `DecodedMessage.number` survives tail/summary trimming; `reference()`
returns the full thread/number plus immutable message ID. `MessageRef` parses raw
IDs and `<thread>#<positive number>`; `resolve_message` selects that exact message,
while `resolve_thread` normalizes it (or a raw reply ID) to the root. Resolution
uses current verified thread history without advancing its read cursor. Signed
sequence/coverage data and the wire protocol are unchanged.

`MessagePerspective` distinguishes the exact device+handle (`You`), another
session of the same person (`YourOtherSession`), and other people (`Other`). Text
views omit the foreign-content wrapper only for verified own-person content;
verification failures remain visible. Summary headings use the summary message's
thread number and the number of uncovered messages shown below.

`wait` still verifies/caches events but omits messages and reactions from its own
exact device+handle, advancing past them and continuing to the original deadline.
Other handles/devices remain visible. Settings/revocations lack handles and are
retained. `format_wait_entry` prints typed message/reply/reaction lines with full
thread/message references, raw IDs and a read pointer. `wait_text` resolves and
renders the batch before committing its cursor, so failed lookups can be retried.
Neither operation marks a thread read.
MIME guessing recognizes source/config extensions and falls back to plain text
for unrecognized UTF-8 text without binary controls.
