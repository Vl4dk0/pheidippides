# Frozen spaces

Each directory here is a space recorded with the **released** binaries of one
version. `tests/compat.rs` opens it with the current code and must keep passing:
that is the promise that an upgrade never asks people to recreate a space or
send invites again.

Layout of `space-<version>/`:

- `server/phi.db`: the server database, served by the current `phi-server`.
- `<person>/config/profiles/<dir>/{profile.toml,keys}` and
  `<person>/cache/<dir>/cache.db`: a client profile exactly as the CLI wrote it
  (`Store::new(config, cache)`). The test only rewrites `server` in
  `profile.toml` to the test server's address and renames the profile directory
  to match, because the directory name hashes the server URL.
- `manifest.json`: ids the test asserts on (threads, the revoked device).

The keys are throwaway test keys. The server was recorded with
`allow_space_creation = "anyone"` and no admin token.

## What `space-0.2.0` contains

Recorded with `ghcr.io/vl4dk0/phi-server:0.2.0` and `phi 0.2.0` (linux x86_64),
by `make-space-fixture.sh`:

1. alice creates `team`; bob joins by invite.
2. `#all`: alice posts, bob replies and acks, alice replies with a summary.
3. alice creates `#backend` with bob; both post. bob sends alice a DM.
4. alice sets `summary_every` to 5 (signed settings, `phi settings v2`).
5. alice enrolls a laptop (`phi device add`); the laptop posts in `#all`;
   alice revokes the laptop (`phi revocation v2` with a history root).
6. Everyone reads everything, so the caches hold the whole history.

## Recording the next one

Record a new fixture only for a **release** that is already published, never
for a working tree, and add a test case for it instead of replacing the old one:
older fixtures must keep passing too.

```sh
# needs Docker; writes crates/phi-core/tests/fixtures/space-<version>/
crates/phi-core/tests/fixtures/make-space-fixture.sh 0.3.0
```

The script pulls the server image and the CLI tarball of that version, runs the
scenario above in containers, vacuums the databases and copies the result here.
Then add the new directory to `compat.rs` and commit both.
