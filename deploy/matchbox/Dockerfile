FROM rust:1.95-slim AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install matchbox_server --locked

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/cargo/bin/matchbox_server /usr/local/bin/matchbox_server

CMD ["sh", "-c", "matchbox_server 0.0.0.0:${PORT:-3536}"]
