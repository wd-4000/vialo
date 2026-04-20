# syntax=docker/dockerfile:1
# Make note of the debian version used by Rust!
FROM rust:1.95-alpine AS builder
RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static

WORKDIR /usr/src/vialo-api

# I Do Not Care
ENV RUSTFLAGS=-Awarnings
ENV OPENSSL_DIR=/usr
ENV OPENSSL_STATIC=1

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/vialo-api/target \
    mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src


COPY . .

# Build it for real
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/vialo-api/target \
    SQLX_OFFLINE=true cargo build --release && \
    cp target/release/vialo-api /vialo-api
# We use cached SQLX queries (SQLX_OFFLINE=true).
# This means you need to run `cargo sqlx prepare` to update the cache!

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /vialo-api /usr/local/bin/vialo-api
WORKDIR /app

ENTRYPOINT ["vialo-api"]
