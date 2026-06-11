# syntax=docker/dockerfile:1

# ── Multi-stage build for the openlet-server binary ──────────────────
# Stage layout: cargo-chef caches the dependency build so source-only
# changes don't re-compile the whole dependency graph; the release build
# runs on the cached deps; the runtime image is a slim, non-root Debian
# carrying only the binary + a healthcheck.

# Build with the latest stable Rust to mirror `rust-toolchain.toml`
# (`channel = "stable"`, the same toolchain CI's `rustup show` resolves).
# Do NOT pin to the workspace `rust-version` (1.85) — that is the MSRV floor,
# and modern build tooling (cargo-chef) needs a newer compiler than the floor.

# --- planner: compute the dependency recipe -------------------------------
FROM rust:slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- builder: cook deps (cached), then build the binary -------------------
FROM chef AS builder
# System deps for the build: pkg-config + libssl headers are commonly
# needed by transitive crates; curl is required by utoipa-swagger-ui's
# build script, which downloads the Swagger UI assets at compile time.
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
# Cook ONLY dependencies — this layer is cached until Cargo.lock/Cargo.toml
# change, so editing source code skips the slow dependency rebuild.
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin openlet-server \
    && strip target/release/openlet-server

# --- runtime: slim non-root image ----------------------------------------
FROM debian:bookworm-slim AS runtime
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
