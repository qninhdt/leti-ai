# syntax=docker/dockerfile:1

# ── Multi-stage build for the openlet-server binary ──────────────────
# Stage layout: cargo-chef caches the dependency build so source-only
# changes don't re-compile the whole dependency graph; the release build
# runs on the cached deps; the runtime image is a slim, non-root Debian
# carrying only the binary + a healthcheck.

# Pin the build image to the SAME toolchain `rust-toolchain.toml` selects
# (`channel = "1.96.1"`), which is also what CI's `rustup show` resolves.
# This must be >= the workspace `rust-version` MSRV floor (1.95, the Monty
# floor); an older base (the previous `1.88`) is below the floor and fails
# to compile. Keep this in lockstep with `rust-toolchain.toml`.

# --- planner: compute the dependency recipe -------------------------------
FROM rust:1.96.1-slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- builder: cook deps (cached), then build the binary -------------------
FROM chef AS builder
# System deps for the build:
#  - protobuf-compiler (protoc) + libprotobuf-dev: openlet-adapters/build.rs
#    compiles the vendored file-service proto into the cloudfs gRPC client via
#    tonic-build. libprotobuf-dev ships the well-known-type includes
#    (`google/protobuf/timestamp.proto`) the vendored proto imports — protoc
#    alone (with --no-install-recommends) does not carry them.
#  - curl: required by utoipa-swagger-ui's build script, which downloads the
#    Swagger UI assets at compile time.
# (The tree is all-rustls — no OpenSSL — so pkg-config/libssl-dev are NOT
# installed; see Cargo.toml "no OpenSSL in the tree".)
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl protobuf-compiler libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
# Cook ONLY dependencies — this layer is cached until Cargo.lock/Cargo.toml
# change, so editing source code skips the slow dependency rebuild.
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin openlet-server \
    && strip target/release/openlet-server

# --- runtime: slim non-root image ----------------------------------------
# Must match the builder's glibc: `rust:slim` is Debian trixie (glibc 2.39),
# so the runtime base is trixie-slim too — a bookworm runtime (glibc 2.36)
# can't load a trixie-built binary (`GLIBC_2.39 not found`).
FROM debian:trixie-slim AS runtime
# ca-certificates for outbound TLS to OpenRouter; curl for the healthcheck.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 openlet

COPY --from=builder /app/target/release/openlet-server /usr/local/bin/openlet-server

# Data root is a volume so sqlite/artifacts/sessions persist across
# container restarts. Owned by the non-root runtime user.
ENV OPENLET_DATA_DIR=/data
RUN mkdir -p /data && chown openlet:openlet /data
VOLUME ["/data"]

# Bind to all interfaces INSIDE the container; host exposure is controlled
# by Compose port mapping (loopback by default). Non-loopback inside the
# container is gated by OPENLET_ALLOW_NON_LOOPBACK at the app layer.
ENV OPENLET_BIND=0.0.0.0:8787 \
    OPENLET_ALLOW_NON_LOOPBACK=1 \
    OPENLET_LOG_FORMAT=json \
    RUST_LOG=info

EXPOSE 8787
# Optional metrics port (Phase 10). Only served when OPENLET_METRICS_BIND
# is set; documented here for operators who enable it.
EXPOSE 9464

USER openlet

# Liveness: the readiness endpoint. Interval/timeout tuned for a fast Rust
# server; start-period covers first-boot migrations.
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8787/v1/health || exit 1

ENTRYPOINT ["openlet-server"]
CMD ["serve"]
