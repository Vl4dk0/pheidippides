# Progress

## Goal

Working **prototype** by morning, for review with Vladko. Prototype = expect heavy rewrites,
features may be removed if not well thought through, complexity may be added later.
Linux/home-server deploy: out of scope (needs sudo — do together later).

**Status: reached.** Server + CLI + E2E crypto + skill + GUI (build-only) on `main`,
fmt/clippy clean, 55 Rust + 6 GUI tests green. Three live multi-agent runs on this Mac passed.

## Milestones

| Milestone | Worker | Result |
|---|---|---|
| Spec | orchestrator (Opus) | `SPEC.md` (later edits marked "changed in prototype review") |
| M1 proto + crypto | Codex (gpt-6.1-sol) | 26456c9 |
| M4 skill + README + deploy | Sonnet | 81aec99 |
| M2 server | Codex | 63effb1 |
| Review: crypto | Grok → Codex | 8 findings (1 critical, 4 high) all fixed, c1f286d |
| M3 core + CLI | Codex | 7a52e7a |
| Review: server | Grok → Codex | 6 fixed, 1 rejected with reason, 250b0b4 |
| M5 GUI (Tauri 2 + Svelte 5) | Codex | c6eadf3 — compiles, never launched |
| Live test 1 → usability fixes | Haiku + Luna → Codex | 8 fixes, 55f5ab9 |
| Review: client + GUI | Grok → Codex | 3 high + 3 medium fixed, f1b1694 |
| Review: simplicity | Codex (gpt-6-astra) → Codex | 13 items + live test 2 UX, f30c001 |
| Live test 3 | Haiku ×2 + Luna | passed, ratings 7–10/10 |

## Live tests (local phi-server on the Mac)

Agents: Claude Haiku as `vladko/main`, a second Haiku session as `vladko/reviewer`,
Codex Luna as `bob/main`. Each run: create space → invite (file = "out of band") →
join → request with attachment → claim → verified reply with attachment → ack →
request to own other session → claim → review → apply. All three runs completed
the task correctly with no human input. Setup 6/10 → 8/10, join 8 → 9.

## Open questions for the morning review

1. **Wake rule for my other sessions.** `wait --as reviewer` wakes on my own
   outgoing traffic to Bob (I'm a member of that DM). Proposal: don't wake on
   messages authored by your own person unless they are addressed to your handle.
2. **Read state is per person** (spec §6.3). A session that reads a thread marks it
   read for all my sessions. Keep it, or switch to per handle?
3. **`wait` returns one event, then the agent runs `read`.** Agents asked for the reply
   body inline. Proposal: print the body of new messages in `wait` (with the
   untrusted-content framing).
4. **Protocol major = crate major (0 for the prototype).** Rule from §6.10; ok?
5. **`phi device add --gui`.** A person's first GUI device can be enrolled from any of
   their devices; later GUI devices only from a GUI device. The "sent from GUI" mark is best effort.
6. **Summary blur.** Summaries written by another agent say "verified …". Agents
   double-checked attachments themselves, which is good. Nothing to change?
7. **Not verified:** ACME/TLS, Docker image (no Docker daemon), Windows/Linux builds,
   GUI runtime (by design), home-server deploy.
