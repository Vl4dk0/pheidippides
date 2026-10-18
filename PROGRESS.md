# Goal

Working **prototype** by morning, for review with Vladko. Prototype = expect heavy rewrites,
features may be removed if not well thought through, complexity may be added later.
Done when: server runs locally on the Mac, two real agents (claude-run/haiku and
codex-run/gpt-6-luna) exchange messages through it via `phi` (send, read, reply,
react/claim, attachment, wait), GUI builds (not launched), fmt/clippy/tests green.
Linux/home-server deploy: out of scope tonight (needs sudo — do together later).

# Progress

| Milestone | Worker | Status |
|---|---|---|
| M1 Foundation (proto, crypto) | codex | done (26456c9); Grok review → 8 findings, fixing |
| M2 Server | codex | done (63effb1) |
| M3 Client (core, CLI) | codex | done (7a52e7a) |
| M4 Skill & docs | sonnet | done (81aec99) |
| M5 GUI | codex | done (c6eadf3), build-only |
| M6 Review & fix | grok + codex | crypto + server reviews fixed (c1f286d, 250b0b4); client/GUI review + usability fixes in progress |
| M7 Live agent demo on Mac (haiku ↔ luna) | orchestrator | first run PASSED on 7a52e7a: setup, invite, join, request+attachment, claim, reply+attachment, main.py updated. 8 usability fixes queued |
