# Deploying phi-server

One `phi-server` process plus one data directory hosts many spaces. The server
stores only ciphertext and routing metadata, so a compromised host leaks
metadata, not messages. Start from `server.example.toml`.

## 1. PC or private network

Run on your own machine, reachable over LAN/VPN (Tailscale, WireGuard, ...).

- `tls.mode = "off"`, `listen` set to the VPN/LAN interface address.
- Plain HTTP is acceptable only because the network is private. Message content
  is end-to-end encrypted regardless.

## 2. Home server with a domain

The server terminates TLS itself.

- `tls.mode = "acme"` with `domain` and `email`; forward public 443 to the configured listener (TLS-ALPN-01). Port 80 is not
  required. Use `listen = "0.0.0.0:443"` or forward 443 to 8443.
- Keep `allow_space_creation = "token"` and set `admin_token`, otherwise the
  public server is an open relay for anyone who finds it.
- `phi-server.service` is a systemd unit: copy the binary to `/usr/local/bin`,
  the config to `/etc/phi/server.toml`, then `systemctl enable --now phi-server`.
  Create the `phi` user first.

## 3. Company behind a reverse proxy

`tls.mode = "off"` on localhost; the proxy terminates TLS. See
`Caddyfile.example` and `nginx.example.conf`. The one non-obvious setting: proxy
read timeouts must be at least 960 s, because `phi wait` and the GUI long-poll
for up to 900 s.

## Docker

```sh
docker build -f deploy/Dockerfile -t phi-server .
docker run -d --name phi -p 8443:8443 \
  -v phi-data:/data -v "$PWD/server.toml:/etc/phi/server.toml:ro" phi-server
```

Set `data_dir = "/data"` in the mounted config. The container runs as a
non-root user.

## Backups

```sh
phi-server backup /path/to/dest-dir
```

The destination must not exist. Takes an online SQLite backup and copies the
blobs under a database writer lock, so writes briefly wait for a consistent
snapshot. Config resolution is the same as normal startup (`--config` or
`PHI_SERVER_CONFIG`). Everything is ciphertext, so
backups may go to untrusted storage. Members' keys live on their own devices; a
server backup does not contain them and cannot restore anyone's ability to read
history.

## Upgrades

Semantic versioning. Client and server **major** versions must match: a server
rejects other majors with HTTP 426 and `phi` tells the user to upgrade. Minor
and patch releases are wire-compatible, so they can be rolled out in any order.
Older majors are never supported; on a major bump upgrade the server and all
clients together. Take a backup first.
