# Deploying phi-server

One process and one data directory host many spaces. Choose a mode below; all
use the same binary and config. The server stores ciphertext and routing metadata.

## Initialize once

Install `phi-server` natively with `phi-server-installer.sh` / `.ps1` from the
[latest release](https://github.com/Vl4dk0/pheidippides/releases/latest) (same
usage as the CLI installer), even when Compose runs the server: init only needs
the binary.

```sh
phi-server init --dir ./phi
cd phi
```

This creates a commented `server.toml` (0644) and a random 256-bit admin token in
`.env` (0600 on Unix). Existing files are never overwritten. With no `--dir`,
init uses the current directory. The checked-in [`server.toml`](server.toml) is
the same template, but contains no token; use init to generate a ready deployment.
Keep `.env` private and out of version control/backups shared with other people.
The server does not load dotenv files automatically. Native shell startup is:

```sh
set -a; . ./.env; set +a
phi-server --config server.toml
```

In another shell, load `.env` the same way, then create your first space:

```sh
PHI_ADMIN_TOKEN="$PHI_SERVER_ADMIN_TOKEN" phi space create game --server https://phi.example.org
```

Use your actual URL (e.g. `http://127.0.0.1:8443` for local use). Space creation
is token-protected by default; invitations govern joining existing spaces.
`PHI_SERVER_ADMIN_TOKEN` is the server setting; `PHI_ADMIN_TOKEN` is the client's
matching credential. Existing config `admin_token` still works, with the env
setting taking precedence.

## 1. PC / private network

The generated config uses plain HTTP on `127.0.0.1:8443` and stores state in
`./data`. For LAN/VPN use, change `listen` to that interface's address. Keep
`tls.mode = "off"` only on the private network; bearer tokens require a protected
transport even though message content is end-to-end encrypted.
`allow_space_creation = "anyone"` is optional on a private network and needs no
admin token. Run natively with the shell commands above.

## 2. Home server with a domain

For an existing nginx host, use the reverse-proxy recipe below, including with
rootless Docker. Alternatively phi-server can terminate TLS itself: set
`[tls] mode = "acme"`, `domain = "phi.example.org"`, `email = "you@example.org"`,
and forward public port 443 directly to its listener on 8443 (TLS-ALPN-01).
Port 80 is not needed for native ACME. Preserve the ALPN handshake; do not put an
HTTP TLS-terminating proxy in front of native ACME. Keep space creation in token
mode. ACME state lives in `data_dir/acme`.

For a native system service, create the `phi` system user/group, copy the binary
to `/usr/local/bin/phi-server`, and install the generated files as
`/etc/phi/server.toml` (0644) and `/etc/phi/.env` (root-owned, 0600). Install
[`phi-server.service`](phi-server.service) in `/etc/systemd/system/`, then run
`sudo systemctl daemon-reload` and `sudo systemctl enable --now phi-server`.
The unit loads the private env file before switching users and overrides the
state path to `/var/lib/phi-server`, which systemd creates with the right owner.
It works behind nginx or with native ACME after the config edits above.

## 3. Behind a reverse proxy (home or company)

Keep TLS off in phi-server. Run natively on localhost, or use Compose:

```sh
cd phi   # the directory created by init
curl -fsSLO https://raw.githubusercontent.com/Vl4dk0/pheidippides/v0.2.0/deploy/compose.yml
docker compose pull
docker compose up -d
```

Compose runs the published multi-arch (amd64/arm64) image
`ghcr.io/vl4dk0/phi-server:0.2.0` by default; the tag is explicit so upgrades
are deliberate. Select another tag or your own build with
`PHI_TAG=0.1.1 docker compose pull` /
`PHI_IMAGE=registry.example.org/team/phi-server` (or put these variables in
`.env`). To build locally instead, run
`docker build -f deploy/Dockerfile -t phi-server:0.2.0 .` from the source
checkout and set `PHI_IMAGE=phi-server`.
The `PHI_SERVER_` prefix is reserved for server config overrides; image variables
use `PHI_IMAGE`/`PHI_TAG` so they can safely share the Compose env file.
The container runs as UID 10001, uses a named `phi-data` volume at `/data`, and
publishes **127.0.0.1:8443 only**. Compose overrides the internal listener to
`0.0.0.0:8443` so the host proxy can reach it. The config bind mount is read-only.

Keep `server.toml` readable (0644): a container UID cannot read a host user's
0600 file, especially with rootless UID mapping. The token stays in the separate
0600 `.env`; Compose reads it on the host and supplies it through `env_file`, so
the container never needs to read that file. A named volume avoids host bind-mount
ownership problems for data. Do not use `docker compose down -v` unless deleting
all server data is intended.

Copy [`nginx.phi.conf`](nginx.phi.conf) into your nginx site's configuration,
replace `phi.example.org`, enable the site, and validate/reload nginx. It starts
with a working HTTP listener so certificate issuance does not require existing
certificate paths. With DNS pointing to the host and ports 80/443 reachable, use
[`certbot --nginx`](https://eff-certbot.readthedocs.io/en/stable/using.html#nginx):

```sh
sudo nginx -t
sudo systemctl reload nginx
sudo certbot --nginx -d phi.example.org
```

Select HTTPS redirection. Expose the service to clients through HTTPS after
certificate setup. For an existing managed certificate, integrate the snippet's
location/body settings into that HTTPS virtual host instead. Caddy users can use
[`Caddyfile.example`](Caddyfile.example) for automatic TLS.

Proxy read/send timeouts are 960 seconds because long-polls can wait 900 seconds;
HTTP/1.1 and disabled response buffering deliver events promptly. Forwarded
headers preserve host/protocol/client context; phi-server authenticates bearer
tokens and does not use those headers as identity. The 40 MiB body limit covers
a default 25 MiB attachment plus AEAD tag and JSON/base64 expansion (about
33.4 MiB). Raise proxy limits when increasing `limits.max_blob_bytes`.

### Rootless Docker on Ubuntu 24.04

Use Docker's [rootless setup](https://docs.docker.com/engine/security/rootless/)
under the deployment user's own SSH login. Install the rootless prerequisites
(including subordinate UID/GID ranges) and `docker-ce-rootless-extras`, then:

```sh
dockerd-rootless-setuptool.sh install
docker context use rootless
systemctl --user enable --now docker
sudo loginctl enable-linger "$(whoami)"
```

Linger keeps the user daemon running across logout/reboot; `unless-stopped`
restarts the container when that daemon starts. Do not join the `docker` group:
access to the rootful Docker socket is effectively passwordless root. Rootless
Docker uses the user's socket/context instead; nginx can remain a host service
on 80/443 and proxy to the unprivileged loopback port 8443.

Ubuntu 24.04 restricts unprivileged user namespaces via AppArmor. The packaged
rootlesskit path is normally covered; a script-installed binary under
`$HOME/bin/rootlesskit` needs its own profile. Follow Docker's
[AppArmor instructions](https://docs.docker.com/engine/security/rootless/troubleshoot/#distribution-specific-hint)
for the exact installed path, reload AppArmor, then restart the user Docker
service. Grant `userns` to rootlesskit with that profile rather than disabling
the host's namespace restriction globally.

## Health, backups, upgrades

`GET /healthz` returns `{"status":"ok","protocol_major":0}`; `HEAD /healthz`
returns the same status without a body. Both need neither auth nor `Phi-Protocol`,
even when a mismatching protocol header is supplied. Other endpoints retain
protocol/auth checks. This is a lightweight process-liveness probe, not a database
or storage audit. Check it from the host, e.g. `curl -fsS http://127.0.0.1:8443/healthz`.
The runtime image has no HTTP probe tool, so Compose deliberately has no
healthcheck; no curl package is added.

```sh
# Native: load .env and use the same config as the running server.
phi-server --config server.toml backup /path/to/new-backup
# Compose: the destination is inside the named data volume.
docker compose exec phi-server phi-server backup /data/backup-2026-10-20
```

The destination must not exist. Backup uses SQLite online backup and copies blobs
under a database writer lock for consistency; writes briefly wait. Copy the
finished backup off the volume (e.g. `docker compose cp phi-server:/data/backup-2026-10-20 ./backup-2026-10-20`).
Backups contain ciphertext and routing metadata, not members' private keys.
Back up `.env` separately and privately if preserving the admin token is desired.

Take a backup before upgrading, then run
`PHI_TAG=<new-version> docker compose pull` and
`PHI_TAG=<new-version> docker compose up -d` (or set `PHI_TAG` in `.env`). From
0.2.0 on, a newer server opens the existing database and members keep their
profiles; CI checks this against a space recorded with the 0.2.0 binaries
(SPEC 6.10). Client/server **major** versions must match or API requests get
HTTP 426. A major bump is announced one release ahead in the changelog and
comes with migration notes; the backup above is what you restore if the
upgrade goes wrong.
