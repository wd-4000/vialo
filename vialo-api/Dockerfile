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
COPY vialo-common/Cargo.toml vialo-common/
COPY vialo-dhcp/Cargo.toml vialo-dhcp/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/vialo/target \
    mkdir -p vialo-api/src vialo-common/src vialo-dhcp/src && \
    echo "fn main() {}" > vialo-api/src/main.rs && \
    touch vialo-common/src/lib.rs && \
    echo "fn main() {}" > vialo-dhcp/src/main.rs && \
    cargo build --locked --profile $PROFILE --features migrate -p vialo-api && \
    rm -rf vialo-api/src vialo-common/src vialo-dhcp/src

COPY . .

# Build it for real. The `touch` is load-bearing: COPY preserves the host's
# mtimes, which are older than the dummy artifacts just built above, so cargo
# would call the empty vialo-common fresh and link that instead. (vialo-api
# itself escapes this only by having a build.rs the dummy layer lacks.)
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/vialo/target \
    TARGET_DIR=$(echo $PROFILE | sed 's/^dev$/debug/') && \
    find vialo-api vialo-common vialo-dhcp -name '*.rs' -exec touch {} + && \
    SQLX_OFFLINE=true cargo build --locked --profile $PROFILE --features migrate -p vialo-api --bin vialo-api && \
    cp target/$TARGET_DIR/vialo-api /vialo-api
# We use cached SQLX queries (SQLX_OFFLINE=true).
# This means you need to run `cargo sqlx prepare` to update the cache!

FROM scratch
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /vialo-api /usr/local/bin/vialo-api
WORKDIR /app
ENV VIALO_CONFIG=/app/vialo.toml

ENTRYPOINT ["vialo-api"]
