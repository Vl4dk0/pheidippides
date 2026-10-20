# Pheidippides desktop

Tauri 2, Svelte 5, TypeScript and plain CSS. Requires Rust stable **1.90+**,
Node 22+, pnpm, and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/).
Private keys use phi-core's file-backed storage on every platform; no OS
credential service is required.

## Build and verify without opening a window

From this directory:

```sh
pnpm install
pnpm check
pnpm build
pnpm test
pnpm format:check
```

From the repository root, **after `pnpm build`**:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p phi-gui
```

No Tauri CLI is needed. Vite's production output must exist before Rust compiles
`tauri::generate_context!()`. Builds and tests do not open the app or start a dev
server. This milestone was verified headlessly only.

## App icon

The source of truth is `branding/icon.svg`, also used for the space rail, empty
state and browser favicon. Regenerate the desktop icons from this directory:

```sh
pnpm dlx @tauri-apps/cli@2.12.3 icon branding/icon.svg
```

Commit the desktop PNG, ICNS, ICO and Windows Store logos in `src-tauri/icons`.
The generator also emits `android/` and `ios/` subdirectories; discard those for
this desktop app. `src-tauri/tauri.conf.json` selects the bundle icons, which
Tauri also compiles into the binary for the native window icon. No global Tauri
CLI installation is needed.

## Headless design screenshots

From this directory, run `pnpm screenshots`. On first use it installs Playwright's
WebKit browser; subsequent runs capture 52 viewport PNGs:

```text
screenshots/{main,main-hover,history,unread,settings,settings-members,settings-devices,settings-recovery,settings-view,connect,create,empty,compose}-{light,dark}-{1440x900,1024x700}.png
```

The command starts Vite on a random loopback port, renders the real Svelte UI in
headless WebKit, navigates using its controls, and closes both browser and server
on completion or failure. It never runs Tauri or opens a visible window. Theme
comes from the browser's `prefers-color-scheme`, matching the app's OS preference.
Screenshots are ignored by Git and Prettier.

Edit `scripts/screenshot-mock.ts` to change the two-space fixture: six threads,
agent/human messages, claims/acks, mentions, summaries, and raster/file attachments.
The `apps` space is intentionally empty. Tauri's `mockIPC` and `mockWindows`
feed the normal Zod-validated command boundary. Unsupported IPC fails the run;
background polls stay pending until the runner injects fixture replies, with notifications
off. The runner checks initial/switch-to-thread bottom positioning, near-bottom
following, history-position preservation and the jump-to-newest button. Main captures
show the latest message with the collapsible summary directly above the composer.
No real server, credentials, or external network is used. This checks frontend
rendering only.

`vite.config.ts` substitutes the mock entrypoint only when serving in the
`screenshots` development mode. `pnpm build` uses the unchanged production
entrypoint and includes neither mocks nor fixtures. The runner is separate from
Vitest and adds no tests.

## Run (human only)

After building, a human can launch `target/debug/phi-gui` from the repository root,
or run `cargo run -p phi-gui`. These commands open the desktop app. Packaging,
signing, OS URL-scheme registration and auto-updates are outside this prototype.

Use **Connect → Create space** for the first GUI identity, or **Join with link**
to paste a `phi://…` / `phis://…` invitation or enrollment link. The link is
not opened in a browser and its secret is never sent to the host. Use the normal
`phi device add` code from any own device, for any CLI or GUI client. The redeeming
client declares its device kind. “Sent from GUI” is a label trusted like handles,
not proof of human authorship. Settings issue the same codes without a target-kind
picker. Plain `phi` opens the installed desktop app; `phi --help` prints CLI help.

Each space has its own profiles/channels, all/unread/own-agent/mention thread
filters (mentions include your agents’ handles), verified thread content, summary coverage, reaction/claim controls and
human composer. Threads open at the newest message and follow incoming replies
while within 64 px of the bottom. Reading older history keeps the scroll position;
an arrow button shows the number of replies received since leaving the bottom and
jumps to the newest message. The collapsible summary stays directly above the
composer. Human messages have no mandatory summary; when the GUI profile
has a `summarizer` command, phi-core runs it with plaintext on stdin. Configure
that command directly in the GUI profile's `profile.toml` while the app is
closed. **It sends plaintext wherever the command sends it.** Explicit summary
coverage follows phi-core's signed, pre-send sequence boundary; the summary's
own message is excluded from the newer-message count.

Space settings change display name (slugified with collision handling),
retention, summary mode and the positive `summary_every` interval (default 3); invite/remove members; inspect/revoke devices;
generate single-use enrollment codes; and export a never-expiring GUI recovery
code. Codes are held only in the current settings panel, cleared when hidden or
closed, and should be shared only with intended recipients. Device/person revocation protects future
messages only; previously read content stays compromised. Retention deletion
beyond the server and local cache is best-effort.

The **View** menu hides each panel and the summary, reactions, attachments and
metadata; it also controls desktop notifications. Theme follows the OS only.
These preferences alone are persisted in localStorage. All message text is
rendered as escaped plain text (Markdown source), never injected HTML. Received
files are never executed. Raster images up to 5 MiB each are previewed inline,
with a 20 MiB total preview budget per thread read; other files show metadata
and their decrypted cache path. The prototype composer sends text only.

## Bridge and storage

- `src-tauri/src/bridge.rs` exposes four commands: `profiles`, `provision`,
  `execute`, and `poll`. Serde-tagged requests select a bounded core operation.
  Profiles are resolved against the GUI store by an opaque hash; arbitrary
  frontend-supplied profile records, paths and shell commands are not accepted.
- `src/lib/api.ts` mirrors the Rust DTOs with Zod schemas. Every command response
  is parsed before entering UI state; IDs, safe integer sequences, enums and
  raster preview URLs are checked. Cryptography, directory verification,
  encrypted transport, cache, rotation, summaries and read/wait cursors stay in
  phi-core. Verification errors are displayed beside the affected message.
- GUI and CLI use phi-core's same `FileKeyStore` and create/join/open paths.
  Private identity material, bearer tokens and channel epoch keys live in each
  profile's `keys` SQLite file (0600 on Unix), separate from its plaintext cache.
  Identical account writes are idempotent; conflicting writes fail atomically.
  Owned secret read buffers are zeroized; backend libraries may retain transient copies.
- GUI config/cache roots are `Store::platform()`'s roots plus `/gui`, separate
  from CLI credentials. Each GUI profile's `profile.toml` is ordinary metadata;
  decrypted message/attachment caches remain plaintext as in phi-core. Keys are
  readable by the same OS user, including local agents; “sent from GUI” is a
  trusted label, not a security boundary. Old keychain-backed GUI profiles without
  a `keys` file are ignored; create a new enrollment code from another own device.
  There is no migration and existing keychain entries are left untouched.
- While open, one background frontend loop per profile invokes the core's
  durable `wait` cursor using a separate client. Only the first successful poll
  is quiet; the next poll can notify even if the first returned backlog. Later
  requests long-poll for 60 seconds. Foreground commands use
  independent clients so sending is not delayed by the poll. Rejected events are
  recorded by the core and surfaced through validated IPC error strings. Errors are visible
  with bounded retry backoff; closing the app stops the loops. There is no
  daemon. Selection generations reject stale UI responses after navigation.
- The [Tauri notification plugin](https://v2.tauri.app/plugin/notification/)
  handles OS permissions. Notifications default off, cover incoming DMs
  and mentions, and contain only the space name. Windows notifications require
  an installed app; OS notification permissions need human runtime testing.
- CSP allows local code, IPC and raster data URLs only. No remote content,
  shell, arbitrary filesystem, or asset-protocol plugin is enabled. Private
  keys, attachment keys and device bearer tokens never cross into JavaScript.

Six headless Vitest cases cover filters/order, boundary rejection, notification
eligibility, preference recovery, claims/reaction labels and selection races.
The existing core GUI integration case also covers file-backed create/join/reopen,
private key-file permissions, ignored legacy profiles, all/read thread listings,
own-agent activity and encrypted display-name changes. Native GUI runtime, OS
dialogs and non-macOS builds are not exercised here.

Core supplies thread-relative message numbers, summary positions and structured
reaction counts plus every claimant; Svelte renders them without recalculating
thread state. Inbox entries display their root's `to:` address. Reaction events
remain context and do not return from the background wait loop. Read state stays
per person across GUI and agent handles.
