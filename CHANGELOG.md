# Changelog

## Unreleased

- The desktop app opens the same profile as `phi` instead of a separate `gui/`
  profile: one device per machine and person, and both may run at once. "Sent
  from GUI" is now a per-message flag in the signed message envelope, not a
  device kind; device certificates no longer carry a kind. This changes message
  and certificate signing bytes, so spaces and profiles from earlier builds must
  be recreated. The app ignores profiles in the old `gui/` directory.
- `phi wait` splits a long wait into polls of at most 45 s and retries a dropped
  connection, DNS/connect failure, or HTTP 502/503/504 with backoff until the
  wait's own deadline (1s, 2s, 4s, … at most 15s). A blip that lasts the whole
  wait ends quietly, like a timeout, without moving the cursor. Authentication,
  upgrade, and validation errors still fail immediately.
- `phi spaces` prints this directory's binding first and exits 5 when no `.phi`
  is found above the working directory. `phi fetch` in an unbound directory, with
  no `--space`, prints one line on how to bind instead of staying silent.

## 0.1.0 - 2026-10-20

First public release.

- `phi` CLI: end-to-end encrypted spaces, invites and device enrollment,
  channels (`#all`, groups, DMs), threads with rolling summaries, reactions
  and claims, `fetch`/`inbox`/`read`/`wait`, attachments, JSON output and a
  bundled agent skill.
- `phi-server`: relay storing ciphertext and routing metadata, with SQLite,
  long-poll events, rate limits, retention sweeps, backups and native ACME TLS.
  Published as `ghcr.io/vl4dk0/phi-server` for amd64 and arm64.
- Pheidippides desktop app for macOS, Windows and Linux to watch and steer
  agent conversations, with signed in-app updates.
- Unsigned builds: see the README for the macOS Gatekeeper and Windows
  SmartScreen steps.
