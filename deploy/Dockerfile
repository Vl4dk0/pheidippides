# Multi-stage so the runtime image carries only the binary, not the toolchain.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p phi-server

FROM debian:bookworm-slim
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
