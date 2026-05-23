---
phase: 1
title: "Foundation"
status: complete
priority: P1
effort: "1w"
dependencies: []
---

# Phase 1: Foundation

> **Amendments apply.** See [amendments-after-red-team.md](./amendments-after-red-team.md) §A,§B,§J,§K,§O and [amendments-plugin-system.md](./amendments-plugin-system.md) §6 (Phase 01) for overrides on trait surface, AppState, config, bind addr, clap structure, and the new `openlet-plugin-api` + `openlet-plugin-registry` crates.

## Overview

Bootstrap the Rust workspace, define the four crates, lock all six adapter traits, and stand up the axum server skeleton with utoipa OpenAPI doc, tracing, and a single GET /v1/health route. No business logic yet — this phase exists so every later phase has a stable surface to plug into.

## Requirements

**Functional:**
- Cargo workspace compiles with `cargo build --workspace`
- `cargo run -p openlet-server` boots axum on configurable port (default 8787)
- `GET /v1/health` returns `{"ok":true,"version":...}`
- `GET /doc/openapi.json` returns valid OpenAPI 3.x via utoipa
- `GET /doc` serves Swagger UI
- Six adapter traits compile in `openlet-core::adapters`
- Tracing JSON logs with `RUST_LOG=info` env filter
- Workspace edition = "2024", MSRV pinned in `rust-toolchain.toml`

**Non-functional:**
- Zero `Box<dyn>` in hot paths — adapters injected as generics on `AppState<P,M,A,T,E,Pm>`
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- Cold build under 90s on dev hardware

## Architecture

**Workspace layout (single repo, four crates):**

```
openlet-ai/
├── Cargo.toml                   # [workspace] members
├── rust-toolchain.toml          # channel = "stable"
├── crates/
│   ├── openlet-core/            # lib — domain types, traits, runtime stubs
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── adapters/        # six trait modules
│   │   │   │   ├── mod.rs
│   │   │   │   ├── model_provider.rs
│   │   │   │   ├── memory_store.rs
│   │   │   │   ├── artifact_store.rs
│   │   │   │   ├── tool_executor.rs
│   │   │   │   ├── event_sink.rs
│   │   │   │   └── permission_manager.rs
│   │   │   ├── types/           # SessionId, MessageId, Part, Role, etc.
│   │   │   └── error.rs         # thiserror enums
│   │   └── Cargo.toml
│   ├── openlet-adapters/        # lib — local impls (stubs only this phase)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── openai_compat/   # ModelProvider impl scaffold
│   │   │   ├── sqlite/          # MemoryStore impl scaffold
│   │   │   ├── localfs/         # ArtifactStore impl scaffold
│   │   │   ├── localshell/      # ToolExecutor impl scaffold
│   │   │   ├── bus/             # EventSink impl scaffold
│   │   │   └── config_perm/     # PermissionManager impl scaffold
│   │   └── Cargo.toml
│   ├── openlet-protocol/        # lib — utoipa-derived DTOs (shared HTTP shapes)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   └── dto/             # SessionDto, MessageDto, EventDto, etc.
│   │   └── Cargo.toml
│   └── openlet-server/          # bin — axum router, AppState, main()
│       ├── src/
│       │   ├── main.rs
│       │   ├── app_state.rs
│       │   ├── router.rs
│       │   ├── routes/
│       │   │   └── health.rs
│       │   └── openapi.rs       # utoipa::OpenApi derive aggregator
│       └── Cargo.toml
└── .gitignore
```

**Why four crates (not three, not five):**
- `openlet-core` owns trait definitions + domain types — must compile WITHOUT any IO crate so it stays test-friendly and embeddable.
- `openlet-adapters` is the single home for ALL six local impls. Per user correction, no local/cloud split — that's a YAGNI tax.
- `openlet-protocol` exists ONLY because utoipa derives leak `utoipa::ToSchema` bounds into types. Keeping HTTP DTOs separate from `openlet-core` lets core stay HTTP-agnostic (matches opencode's `packages/sdk` boundary).
- `openlet-server` is the binary. Holds axum router + `AppState`.

**Adapter traits (locked surface, no impls this phase):**

```rust
// openlet-core/src/adapters/model_provider.rs
#[async_trait]
pub trait ModelProvider: Send + Sync + 'static {
    type Stream: Stream<Item = Result<ChatDelta, ProviderError>> + Send + Unpin;
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<Self::Stream, ProviderError>;
    fn pricing(&self, model: &str) -> Option<ModelPricing>;
}

// openlet-core/src/adapters/memory_store.rs
#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    async fn create_session(&self, agent_id: &str) -> Result<SessionId, MemoryError>;
    async fn append_message(&self, session: SessionId, msg: Message) -> Result<MessageId, MemoryError>;
    async fn append_part(&self, msg: MessageId, part: Part) -> Result<PartId, MemoryError>;
    async fn list_messages(&self, session: SessionId) -> Result<Vec<Message>, MemoryError>;
    async fn get_session(&self, session: SessionId) -> Result<Option<SessionMeta>, MemoryError>;
}

// openlet-core/src/adapters/artifact_store.rs
#[async_trait]
pub trait ArtifactStore: Send + Sync + 'static {
    async fn put(&self, session: SessionId, key: &str, bytes: Bytes) -> Result<ArtifactRef, ArtifactError>;
    async fn get(&self, r: &ArtifactRef) -> Result<Bytes, ArtifactError>;
    async fn list(&self, session: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError>;
}

// openlet-core/src/adapters/tool_executor.rs
#[async_trait]
pub trait ToolExecutor: Send + Sync + 'static {
    async fn run_bash(&self, ctx: ToolCtx, cmd: BashCommand) -> Result<BashOutput, ToolError>;
    async fn read_file(&self, ctx: ToolCtx, path: &Path) -> Result<FileBlob, ToolError>;
    async fn write_file(&self, ctx: ToolCtx, path: &Path, bytes: Bytes) -> Result<(), ToolError>;
    async fn list_dir(&self, ctx: ToolCtx, path: &Path) -> Result<Vec<DirEntry>, ToolError>;
    async fn glob(&self, ctx: ToolCtx, pattern: &str) -> Result<Vec<PathBuf>, ToolError>;
    async fn grep(&self, ctx: ToolCtx, args: GrepArgs) -> Result<Vec<GrepHit>, ToolError>;
}

// openlet-core/src/adapters/event_sink.rs
#[async_trait]
pub trait EventSink: Send + Sync + 'static {
    async fn publish(&self, ev: AgentEvent) -> Result<(), EventError>;
    fn subscribe(&self, filter: EventFilter) -> BroadcastStream<AgentEvent>;
}

// openlet-core/src/adapters/permission_manager.rs
#[async_trait]
pub trait PermissionManager: Send + Sync + 'static {
    async fn check(&self, ctx: PermissionCtx, req: PermissionRequest) -> Result<Decision, PermissionError>;
    async fn reply(&self, ask_id: AskId, decision: Decision) -> Result<(), PermissionError>;
}
```

`AppState` is generic over the six adapters: `pub struct AppState<P,M,A,T,E,Pm>` — monomorphized, zero dyn cost. `main.rs` builds it once with concrete impls (stubs in this phase).

**Tracing setup** (`openlet-server/src/main.rs`):
```rust
tracing_subscriber::registry()
    .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
    .with(tracing_subscriber::fmt::layer().json())
    .init();
```

**utoipa skeleton** (`openlet-server/src/openapi.rs`): aggregates `#[derive(OpenApi)]` with `paths(routes::health::handler)` + `components(schemas(HealthResponse))`. Mounted at `/doc` via `SwaggerUi::new("/doc").url("/doc/openapi.json", ApiDoc::openapi())`.

## Related Code Files

**Create:**
- `Cargo.toml` (workspace root)
- `rust-toolchain.toml`
- `.gitignore`
- `crates/openlet-core/{Cargo.toml,src/lib.rs,src/error.rs}`
- `crates/openlet-core/src/adapters/{mod.rs,model_provider.rs,memory_store.rs,artifact_store.rs,tool_executor.rs,event_sink.rs,permission_manager.rs}`
- `crates/openlet-core/src/types/{mod.rs,session.rs,message.rs,part.rs,event.rs,permission.rs}`
- `crates/openlet-adapters/{Cargo.toml,src/lib.rs}` + six module skeletons
- `crates/openlet-protocol/{Cargo.toml,src/lib.rs,src/dto/mod.rs}`
- `crates/openlet-server/{Cargo.toml,src/main.rs,src/app_state.rs,src/router.rs,src/openapi.rs,src/routes/health.rs}`
- `README.md` (root) — quickstart + crate map

**Modify:** none (greenfield).

**Delete:** none.

## Implementation Steps

1. **Workspace skeleton.** Run `cargo new --lib crates/openlet-core` then equivalents for the other three. Hand-edit root `Cargo.toml` to declare `[workspace] members = [...]` and `resolver = "2"` (required for 2024 edition behaviour). Pin `rust-toolchain.toml` to a recent stable.
2. **Crate dependencies.** Per `research/researcher-rust-crates.md` final manifest:
   - `openlet-core`: tokio (narrow features), tokio-util, async-trait, serde, serde_json, thiserror, chrono, uuid, bytes, futures
   - `openlet-adapters`: openlet-core + sqlx, reqwest, schemars (no axum dep!)
   - `openlet-protocol`: serde, utoipa, schemars, chrono, uuid (no IO deps)
   - `openlet-server`: openlet-{core,adapters,protocol} + tokio (full), axum, tower, tower-http, utoipa-axum, utoipa-swagger-ui, tracing-subscriber, anyhow
3. **Domain types** in `openlet-core::types`. Keep them PURE — no `ToSchema` derives here (those live in `openlet-protocol`). `SessionId`/`MessageId`/`PartId` are `Uuid` newtypes with `serde` + `From`/`Display`.
4. **Define six adapter traits** as in Architecture. Each trait file ≤120 lines. Use `async_trait` for now (revisit native async-fn-in-trait once stable on the toolchain).
5. **Error types** (`openlet-core/src/error.rs`). One `enum` per trait module: `ProviderError`, `MemoryError`, `ArtifactError`, `ToolError`, `EventError`, `PermissionError`. All `#[derive(thiserror::Error, Debug)]`. Add a top-level `pub enum CoreError` that `From<...>` each subordinate.
6. **Stub adapter impls** in `openlet-adapters/src/{openai_compat,sqlite,localfs,localshell,bus,config_perm}/mod.rs`. Each defines a unit struct (`pub struct OpenAiCompatProvider;`) and an empty `impl` block. They compile but every method returns `Err(...::Unimplemented)`.
7. **Protocol DTOs.** Just `HealthDto` + an empty `dto/mod.rs` for now. Add `#[derive(utoipa::ToSchema)]`. Subsequent phases extend this.
8. **AppState.** Generic struct in `openlet-server/src/app_state.rs`:
   ```rust
   pub struct AppState<P,M,A,T,E,Pm> { provider: Arc<P>, memory: Arc<M>, ... }
   ```
   `Clone` via `Arc` fields.
9. **Router** (`openlet-server/src/router.rs`). Build with `OpenApiRouter::new()` from utoipa-axum, register `routes::health` via `routes!(health::handler)`, fold into the main `Router<AppState>`.
10. **Health route.** `async fn handler(State(_): State<AppState<...>>) -> Json<HealthDto>` returning version from `env!("CARGO_PKG_VERSION")`.
11. **main.rs.** Init tracing, parse `OPENLET_PORT` env (fallback 8787), build `AppState` with stub adapters, run `axum::serve(TcpListener::bind(...).await?, router).await`. Honor `Ctrl+C` via `axum::serve(...).with_graceful_shutdown(signal::ctrl_c())`.
12. **README.md** — 30-line quickstart: `cargo run -p openlet-server`, then `curl localhost:8787/v1/health` and `open http://localhost:8787/doc`.
13. **CI smoke** (skip if no CI yet): `cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.

## Reference Cross-Check (MANDATORY before coding)

Spawn parallel exploration subagents on:
- **opencode**: `packages/opencode/src/server/server.ts` (Hono route registration), `packages/opencode/src/session/index.ts` (Session type shape — what fields they expose), `packages/sdk/js/src/gen/types.gen.ts` (what the SDK consumes — informs our DTO surface).
- **claw-code**: `rust/Cargo.toml` (workspace structure pattern), `rust/crates/runtime/src/lib.rs` (their feature gating), `rust/crates/api/src/error.rs` (thiserror layout for HTTP-ish errors), `rust/crates/rusty-claude-cli/src/main.rs` (tracing-subscriber setup).

Confirm or revise: trait shapes, error split, AppState generic-vs-trait-object decision, port number convention.

## Success Criteria

- [x] `cargo build --workspace` clean
- [x] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [x] `cargo run -p openlet-server` boots and survives Ctrl+C without panics
- [x] `curl localhost:8787/v1/health` → `200 {"ok":true,"version":"0.1.0"}`
- [x] `curl localhost:8787/doc/openapi.json` validates as OpenAPI 3.x (run through `swagger-cli validate`)
- [x] `open http://localhost:8787/doc` shows Swagger UI with the health route listed
- [x] All six adapter traits exist in `openlet-core::adapters` and compile
- [x] All six stub impls compile and can be instantiated in `main.rs`
- [x] `RUST_LOG=info,openlet=debug` produces JSON logs
- [x] Cross-check report saved to `research/cross-check-phase-01.md` (per §Q)

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `utoipa-axum` minor mismatch with axum 0.8 | M | M | Pin both at workspace root; verify compile before committing manifest |
| Async-fn-in-trait vs `async_trait` divergence later | M | L | Wrap in `async_trait` now; revisit per-trait once we have impls |
| Generic `AppState<P,M,A,T,E,Pm>` makes route signatures ugly | M | M | Add a `pub type DefaultAppState = AppState<OpenAiCompat, Sqlite, LocalFs, LocalShell, Bus, ConfigPerm>;` alias once concrete; route handlers use `State<DefaultAppState>` |
| 2024 edition surprises with `inventory`/linker | L | M | We don't use inventory (per brainstorm §17); manual registration only |
| Tracing JSON layer noisy in dev | L | L | `EnvFilter` lets devs flip to `pretty()` via feature flag if needed |

## Next Steps

Phase 2 (storage + message model) builds on the locked `MemoryStore`/`ArtifactStore` traits and the `Part`/`Message` types defined here.
