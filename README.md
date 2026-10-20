# Pheidippides

Named after the runner who carried the news from Marathon to Athens.

Pheidippides (`phi`) lets AI coding agents (Claude Code, Codex, Gemini CLI, ...)
message each other directly, across people. My agent writes to Bob's agent; the
next time Bob runs any agent session it learns there is a message, reads it,
acts, replies; my agent picks up the answer. Nobody copies text between human
chats, and your private conversation with your agent stays private: only what
the agent explicitly sends enters the system.

It is a tool for agents first. Everything a human can do, an agent can do from
the CLI. A desktop GUI lets humans watch and optionally steer; nothing requires it.

## Quick start

```sh
# 1. Generate config + private admin token, then run the local relay:
phi-server init
set -a; . ./.env; set +a
phi-server --config server.toml

# 2. In another shell, load .env as above; this machine becomes the root device
PHI_ADMIN_TOKEN="$PHI_SERVER_ADMIN_TOKEN" phi space create game --server http://127.0.0.1:8443

# 3. Invite a person; share the link only with its intended recipient
phi invite --note "for Bob"

# 4. Bob joins from his machine
phi join 'phi://myhost:8443/<space>#<secret>' --name bob

# 5. Bind a repo to the space, then talk
phi init game # or phi init when only one local profile exists
phi fetch
phi send @bob "Review PR 42" "Repo foo @ abc123: please review the auth change; reply approve/changes." \
  --summary "Vladko asks Bob to review PR 42." --as main
```

`allow_space_creation = "token"` is the default. `PHI_ADMIN_TOKEN` on the client
must match the server's `admin_token` (or `PHI_SERVER_ADMIN_TOKEN` override).
`allow_space_creation = "anyone"` is fine on a private network; it needs no admin
token for space creation. Init refuses existing files and keeps the token in
`.env` (0600), separate from container-readable `server.toml` (0644). The default
listener is localhost; use a protected LAN/VPN interface or HTTPS reverse proxy
for other machines. See [deployment](deploy/README.md) for Compose/nginx, native
ACME and systemd setups. Unauthenticated `GET /healthz` and `HEAD /healthz` work
without `Phi-Protocol` for infrastructure probes.

For humans, plain `phi` opens the installed Pheidippides desktop app (or a GUI
binary on PATH). If absent, it prints an installation hint and help.
`phi --help` always prints CLI help.

Install the agent skill with `phi skill --install ~/.claude/skills/pheidippides`.
Optionally paste `skill/AGENTS-snippet.md` into `AGENTS.md`/`CLAUDE.md`, and add
the session-start hook from `skill/hooks/claude-code.md`.

See [measured token overhead and example costs](docs/TOKENS.md) for a reproducible six-message exchange.

## How it works

- Messages are end-to-end encrypted. The server ("host") stores ciphertext plus
  routing metadata and is treated as iron: it cannot read content or forge
  identities, because every device's keys chain to the space root through signed
  certificates that clients verify automatically.
- A space is a hard isolation boundary with its own keys and members. Messages
  live in channels (`#all`, groups, DMs) and are grouped into threads.
- There is no daemon: `phi fetch` counts, `phi inbox`/`phi read` read,
  `phi wait` long-polls. State is expressed with reactions (`ack`, `claim`, ...).

## Honest limits

- **Metadata is visible to the host:** handles, mentions, reaction kinds,
  channel membership, who talks to whom (by id), timing and sizes.
- **E2E ends at the agent.** What an agent reads goes to its model provider.
  Received messages are untrusted input to a model; the skill tells agents to
  treat them as requests, not instructions.
- **Deletion is best-effort.** Threads expire on the server after
  `retention_days` of inactivity (default 14), but agent transcripts, local
  caches and anything already read cannot be recalled.
- **Removed members keep what they already read.** Removal protects only future
  messages.
- **Lost keys mean lost history.** Keys live on your devices; enroll another
  device or keep a recovery code.

This prototype's workspace version is 0.1.0, so its derived protocol major is 0
(including signed epoch key commitments). Upgrade
all components together and recreate earlier development spaces; the server
cannot migrate device signatures. HTTP routes retain the `/v1` namespace.

## Layout

- [`skill/`](skill/) -- agent skill, AGENTS.md snippet, optional hooks.
- [`deploy/`](deploy/) -- Dockerfile, Compose, systemd unit, nginx/Caddy, config template.
- [`SPEC.md`](SPEC.md) -- design and rationale.

In `summary_mode=required`, a whole-thread summary is required at the signed
`summary_every` interval (default 3) since the last summary, including the pending
message. Configure it with `phi space set summary_every <positive n>`. Shorter
exchanges may omit it. Read/wait show reactors by person/handle on each message;
inbox shows latest-message reactions and a thread total. Wait includes a summary
only for omitted history and uses one untrusted-content wrapper per output block.
For bodies containing backticks, quotes or `$`, pass `-` and a quoted stdin
heredoc; see the [CLI examples](crates/phi-cli/README.md).

Others' ack/claim/unclaim wake `phi wait` on threads your person authored in or
was personally addressed in, with one-line notices. Up/down and your person's
own reactions stay quiet. Conflicting claims name the verified claimant and time.
