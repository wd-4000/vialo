# syntax=docker/dockerfile:1
# Make note of the debian version used by Rust!
FROM rust:1.94.1 AS builder
WORKDIR /usr/src/vialo-api
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/vialo-api/target \
    SQLX_OFFLINE=true cargo install --path .
# We use cached SQLX queries (SQLX_OFFLINE=true).
# This means you need to run `cargo sqlx prepare` to update the cache!

# Make sure the debian version is the same as the one used by Rust!
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/vialo-api /usr/local/bin/vialo-api
WORKDIR /app
CMD ["vialo-api"]
