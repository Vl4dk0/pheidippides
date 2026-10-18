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
| M1 Foundation (proto, crypto) | codex | in progress |
| M2 Server | codex | pending |
| M3 Client (core, CLI) | codex | pending |
| M4 Skill & docs | sonnet | done (81aec99) |
| M5 GUI | codex | pending |
| M6 Review & fix | grok + codex | pending |
| M7 Live agent demo on Mac (haiku ↔ luna) | orchestrator | pending |
