#!/bin/sh
# Record a frozen space with the released binaries of one version, for tests/compat.rs.
# Usage: make-space-fixture.sh <version>      (needs Docker; see README.md)
set -eu
VERSION="${1:?usage: make-space-fixture.sh <version>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$HERE/space-$VERSION"
WORK="$(mktemp -d)"
NET="phifix-$$"
SERVER="phifix-server-$$"
IMAGE="ghcr.io/vl4dk0/phi-server:$VERSION"
TARBALL="phi-cli-x86_64-unknown-linux-gnu.tar.xz"
BASE="https://github.com/Vl4dk0/pheidippides/releases/download/v$VERSION"
trap 'docker rm -f "$SERVER" >/dev/null 2>&1; docker network rm "$NET" >/dev/null 2>&1; rm -rf "$WORK"' EXIT

[ ! -e "$OUT" ] || { echo "$OUT exists; fixtures are never overwritten" >&2; exit 1; }
echo "fetching phi $VERSION"
curl -fsSLo "$WORK/$TARBALL" "$BASE/$TARBALL"
curl -fsSLo "$WORK/$TARBALL.sha256" "$BASE/$TARBALL.sha256"
(cd "$WORK" && sha256sum -c --quiet "$TARBALL.sha256" 2>/dev/null \
    || [ "$(shasum -a 256 "$TARBALL" | cut -d' ' -f1)" = "$(cut -d' ' -f1 "$TARBALL.sha256")" ])
mkdir -p "$WORK/x" && tar -xJf "$WORK/$TARBALL" -C "$WORK/x"
PHI="$(find "$WORK/x" -type f -name phi | head -1)"
docker pull -q --platform linux/amd64 "$IMAGE" >/dev/null

mkdir -p "$WORK/server" "$WORK/alice" "$WORK/bob" "$WORK/alice-laptop"
chmod 777 "$WORK/server"
cat > "$WORK/server.toml" <<'TOML'
data_dir = "/data"
listen = "0.0.0.0:8443"
allow_space_creation = "anyone"
[tls]
mode = "off"
TOML
docker network create "$NET" >/dev/null
docker run -d --name "$SERVER" --platform linux/amd64 --network "$NET" \
    -v "$WORK/server:/data" -v "$WORK/server.toml:/etc/phi/server.toml:ro" "$IMAGE" >/dev/null
sleep 3

# as <home> <phi args...>: one CLI call as that person; USER gives the display name.
as() {
    h="$1"; shift
    docker run --rm --platform linux/amd64 --network "$NET" -e HOME=/home/u -e USER="${h%%-*}" -w /home/u \
        -v "$WORK/$h:/home/u" -v "$PHI:/usr/local/bin/phi:ro" debian:bookworm-slim phi "$@"
}
thread() { sed -n 's/^thread \([^ ]*\).*/\1/p'; }

as alice space create team --server "http://$SERVER:8443"
LINK="$(as alice invite --note bob | tail -1)"
as bob join "$LINK" --name bob
T1="$(as alice --as main send '#all' 'Welcome to team' 'First message in #all. @bob please ack.' | thread)"
as bob --as main reply "$T1" 'Reply from bob in #all.' >/dev/null
as bob --as main react "$T1#1" ack
as alice --as main reply "$T1" 'Third message, with summary.' --summary 'Alice welcomed the team, bob acked and replied.' >/dev/null
as alice --as main group create backend @bob
T2="$(as alice --as main send '#backend' 'Backend plan' 'Private group thread.' | thread)"
as bob --as main reply "$T2" 'bob in #backend.' >/dev/null
T3="$(as bob --as main send '@alice' 'Direct question' 'DM from bob to alice.' | thread)"
as alice --as main space set summary_every 5
CODE="$(as alice --as main device add | tail -1)"
as alice-laptop join "$CODE" --name alice
T4="$(as alice-laptop --as laptop send '#all' 'From the laptop' 'Signed by the device that gets revoked.' | thread)"
LAPTOP="$(sed -n 's/^device_id = "\(.*\)"/\1/p' "$WORK"/alice-laptop/.config/pheidippides/profiles/*/profile.toml)"
as alice --as main device revoke "$LAPTOP"
for who in alice bob; do
    for t in "$T1" "$T2" "$T3" "$T4"; do as "$who" --as main read "$t" >/dev/null; done
done
docker stop "$SERVER" >/dev/null

mkdir -p "$OUT/server"
cp "$WORK/server/phi.db" "$OUT/server/phi.db"
for who in alice bob alice-laptop; do
    mkdir -p "$OUT/$who"
    cp -R "$WORK/$who/.config/pheidippides" "$OUT/$who/config"
    cp -R "$WORK/$who/.cache/pheidippides" "$OUT/$who/cache"
done
find "$OUT" -name '*.db' -exec sqlite3 {} 'PRAGMA journal_mode=DELETE; VACUUM;' \; >/dev/null
find "$OUT" \( -name '*-shm' -o -name '*-wal' \) -delete
printf '{"t_all":"%s","t_backend":"%s","t_dm":"%s","t_laptop":"%s","laptop_device":"%s"}\n' \
    "$T1" "$T2" "$T3" "$T4" "$LAPTOP" > "$OUT/manifest.json"
echo "recorded $OUT"
