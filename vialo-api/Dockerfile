# syntax=docker/dockerfile:1
# This is the local/compose Dockerfile: cache mounts keep target/ between
# builds for fast incremental compiles. CI publishes from Dockerfile.ci.
FROM rust:1.97-alpine AS builder
ARG PROFILE=release

RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static

WORKDIR /usr/src/vialo

# I Do Not Care
ENV RUSTFLAGS=-Awarnings
ENV OPENSSL_DIR=/usr
ENV OPENSSL_STATIC=1

# Cache dependencies: dummy sources make this layer depend on the manifests only
COPY Cargo.toml Cargo.lock ./
COPY vialo-api/Cargo.toml vialo-api/
COPY vialo-dhcp/Cargo.toml vialo-dhcp/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/vialo/target \
    mkdir -p vialo-api/src vialo-dhcp/src && \
    echo "fn main() {}" > vialo-api/src/main.rs && \
    echo "fn main() {}" > vialo-dhcp/src/main.rs && \
    cargo build --locked --profile $PROFILE --features migrate -p vialo-api && \
    rm -rf vialo-api/src vialo-dhcp/src

COPY . .

# Build it for real
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/vialo/target \
    TARGET_DIR=$(echo $PROFILE | sed 's/^dev$/debug/') && \
    SQLX_OFFLINE=true cargo build --locked --profile $PROFILE --features migrate -p vialo-api --bin vialo-api && \
    cp target/$TARGET_DIR/vialo-api /vialo-api
# We use cached SQLX queries (SQLX_OFFLINE=true).
# This means you need to run `cargo sqlx prepare` to update the cache!

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /vialo-api /usr/local/bin/vialo-api
WORKDIR /app

ENTRYPOINT ["vialo-api"]
