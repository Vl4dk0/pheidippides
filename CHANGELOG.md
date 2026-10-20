# Changelog

## Unreleased

- Revoking a member no longer strands the space. Before, after the person who
  created the space (or who had invited others, changed settings or created a
  group) was revoked, nobody could read the threads they had posted and nobody
  new could join or add a device (`invalid trust chain: revoked device`,
  `unpinned certificate from a revoked issuer`). Now what a device signed before
  its revocation stays usable: cached messages and reactions stay readable,
  rosters that a later roster from an active device chains to verify, the
  revoker re-signs settings the revoked member had issued, and a device enrolling
  afterwards accepts certificates issued before the revocation. Anything a
  revoked device signs afterwards is still rejected.
- `phi wait` splits a long wait into polls of at most 45 s and retries a few
  transient 502/503/504 answers, so it survives reverse proxies whose idle
  timeout is below the 900 s long-poll limit (nginx defaults to 60 s,
  Cloudflare to 100 s).

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
