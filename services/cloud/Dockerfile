FROM rust:1.98-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --locked --release --package node2socks-cloud

FROM debian:bookworm-slim
RUN apt-get update && apt-get install --yes --no-install-recommends ca-certificates curl \
    && useradd --system --uid 10001 --home /nonexistent --shell /usr/sbin/nologin node2socks \
    && mkdir /data \
    && chown node2socks:node2socks /data \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/node2socks-cloud /usr/local/bin/node2socks-cloud
USER node2socks
VOLUME ["/data"]
EXPOSE 8080
ENV NODE2SOCKS_CLOUD_DATA=/data
ENTRYPOINT ["/usr/local/bin/node2socks-cloud"]
