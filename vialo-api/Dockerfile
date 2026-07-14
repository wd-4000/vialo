# syntax=docker/dockerfile:1
# This is the local/compose Dockerfile: cache mounts keep target/ between
# builds for fast incremental compiles. CI publishes from Dockerfile.ci.
FROM rust:1.95-alpine AS builder
ARG PROFILE=release

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
    cargo build --profile $PROFILE && \
    rm -rf src


COPY . .

# Build it for real
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/vialo-api/target \
    TARGET_DIR=$(echo $PROFILE | sed 's/^dev$/debug/') && \
    SQLX_OFFLINE=true cargo build --profile $PROFILE --features migrate && \
    cp target/$TARGET_DIR/vialo-api /vialo-api
# We use cached SQLX queries (SQLX_OFFLINE=true).
# This means you need to run `cargo sqlx prepare` to update the cache!

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /vialo-api /usr/local/bin/vialo-api
WORKDIR /app

ENTRYPOINT ["vialo-api"]
