# syntax=docker/dockerfile:1
# Production image, mirroring vialo-api/Dockerfile: cache mounts keep target/
# between builds for fast incremental compiles. There is no .ci variant — this
# server's primary deployment is the Debian package (debian/service), and the
# image exists for compose.dhcp.yaml. Dockerfile.dev is the dev shell container.
FROM rust:1.97-alpine AS builder
ARG PROFILE=release

# No openssl: this crate's sqlx has no TLS feature (see vialo-dhcp/Cargo.toml).
RUN apk add --no-cache musl-dev

WORKDIR /usr/src/vialo

# I Do Not Care
ENV RUSTFLAGS=-Awarnings

# Cache dependencies: dummy sources make this layer depend on the manifests
# only. Every workspace member needs a manifest and a target for cargo to load
# the workspace, even the ones we are not building.
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
    cargo build --locked --profile $PROFILE -p vialo-dhcp && \
    rm -rf vialo-api/src vialo-common/src vialo-dhcp/src

COPY . .

# Build it for real. The `touch` is load-bearing: COPY preserves the host's
# mtimes, which are older than the dummy artifacts just built above, so cargo
# would call the empty vialo-common fresh and link that instead.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/vialo/target \
    TARGET_DIR=$(echo $PROFILE | sed 's/^dev$/debug/') && \
    find vialo-api vialo-common vialo-dhcp -name '*.rs' -exec touch {} + && \
    SQLX_OFFLINE=true cargo build --locked --profile $PROFILE -p vialo-dhcp --bin vialo-dhcp && \
    cp target/$TARGET_DIR/vialo-dhcp /vialo-dhcp
# We use cached SQLX queries (SQLX_OFFLINE=true), in vialo-dhcp/.sqlx.
# This means you need to run `cargo sqlx prepare` in vialo-dhcp/ to update it!

FROM scratch
# Unused today (no TLS feature), but here so enabling one later doesn't produce
# a certificate error out of a scratch image with nothing to debug it with.
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /vialo-dhcp /usr/local/bin/vialo-dhcp
WORKDIR /app
ENV VIALO_CONFIG=/app/vialo.toml

ENTRYPOINT ["vialo-dhcp"]
