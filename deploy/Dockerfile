# Multi-stage so the runtime image carries only the binary, not the toolchain.
# rust:1-bookworm
FROM rust@sha256:e12566d5abc17a0da741628782cdb504b68b8961f5bfcf677936bf3e8b32de29 AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p phi-server

# debian:bookworm-slim
FROM debian@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587
# ca-certificates: ACME mode talks to the certificate authority.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /data phi \
 && mkdir /data && chown phi /data
COPY --from=build /src/target/release/phi-server /usr/local/bin/phi-server

# Never run as root: the server only needs to write its data dir.
USER phi
# All state lives here; mount a volume so upgrades replace the image, not the data.
VOLUME /data
EXPOSE 8443
# Config is mounted at /etc/phi/server.toml (set data_dir = "/data").
ENV PHI_SERVER_CONFIG=/etc/phi/server.toml
ENTRYPOINT ["phi-server"]
