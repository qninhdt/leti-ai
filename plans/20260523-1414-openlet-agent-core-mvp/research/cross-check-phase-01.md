# Cross-check Report — Phase 01 Foundation

**Date:** 2026-05-23
**Sources scouted:** `temp/opencode` (TS/Effect server) + `temp/claw-code` (Rust CLI)

## Summary

Phase 01 architectural decisions hold. No revisions required. Two minor parity gaps surfaced; both are deliberate (plan supersedes).

## Findings

### From opencode

| Question | Finding | Action |
|---|---|---|
| Health response shape | opencode uses `{healthy: true, version}` | Plan locks `{ok: true, version}` — keep plan, deliberate divergence |
| Default bind | `127.0.0.1` (port auto, prefers 4096) | `127.0.0.1:8787` per §K — consistent loopback posture, different port |
| Session DTO fields | `id, slug, projectID, directory, parentID?, agent?, version, time{created,updated,...}, permission?` | Phase 02 SessionMeta — adopt nested `time` substruct for SDK parity |
| Permission mode | opencode embeds full `Permission.Ruleset`; coarse mode does not exist | §A `permission_mode` enum is a deliberate simplification — document in type doc |
| Plugin init timing | Lazy per-instance after server listens, non-fatal on failure | MVP plugin registry is empty in Phase 01 — defer to Phase 03+ when plugins ship |
| Route grouping | Per-feature `HttpApiGroup` modules composed into top-level api | Mirror with per-feature `routes/*.rs` modules + `OpenApiRouter::new()` composition |

### From claw-code

| Question | Finding | Action |
|---|---|---|
| Workspace structure | Resolver "2", edition 2021, `[workspace.package]` inheritance, sparse `[workspace.dependencies]` | **Adopt:** `[workspace.package]` inheritance. **Upgrade:** edition 2024 (plan), hoist tokio/axum/serde/tracing/thiserror to `[workspace.dependencies]` |
| Workspace lints | `unsafe_code = "forbid"`, `clippy.all = warn`, `pedantic = allow` | **Adopt** — strong default |
| Error taxonomy | Hand-rolled enum with `safe_failure_class() -> &'static str` returning closed set (`"context_window"`, `"provider_auth"`, ...) | **Adopt the accessor pattern** — `impl CoreError { pub fn class(&self) -> &'static str }`. Use `thiserror` for boilerplate (claw-code hand-rolls `Display`, we don't need to). Aligns with §S |
| Anti-pattern in claw-code | `Auth(String)` free-form variant in `ApiError` | **Reject** — §S forbids `Other(String)`-style variants |
| CLI parsing | Hand-rolled `parse_args` over `env::args()` | **Reject** — §O specifies clap derive with Serve/Audit subcommands |
| Async runtime | Sync `fn main()` + ad-hoc `Builder::new_current_thread().block_on()` per call | **Reject** — `#[tokio::main]` for the server (long-lived process) |
| Logging | `eprintln!` with manual JSON branching, no `tracing` | **Reject** — `tracing-subscriber` JSON layer per phase-01 §Tracing setup |
| Provider polymorphism | `enum ProviderClient { Anthropic, Xai, OpenAi }` (closed set) | **Reject for AppState** — §B chose `Arc<dyn ModelProvider>` because plugins must add providers at runtime |
| AppState pattern | N/A (CLI has no AppState) | No signal — §B `Arc<dyn _>` decision stands on its own |
| axum / utoipa | N/A (no HTTP) | No signal — follow utoipa-axum upstream docs |

## Decisions confirmed

1. **AppState uses `Arc<dyn _>` (§B).** Both references support the choice: opencode does runtime-pluggable layers; claw-code's enum approach is too rigid for a plugin system.
2. **Default bind 127.0.0.1:8787 (§K).** Consistent with opencode's loopback default.
3. **Six adapter trait names** (`ModelProvider`, `MemoryStore`, `ArtifactStore`, `ToolExecutor`, `EventSink`, `PermissionManager`). Greenfield — neither reference has direct equivalents to copy from.
4. **clap derive with Serve+Audit subcommands (§O).** Reject claw-code hand-rolled args.
5. **Edition 2024 + resolver 2.** Workspace upgrade beyond claw-code's 2021.
6. **Adapter trait surface includes §A additions:** `list_sessions`, `delete_session`, `upsert_part`, `record_read`, `record_always`. Locked in this phase even though impls land in Phase 02-04.
7. **Add `class()` accessor to `CoreError`.** Returns `&'static str`. Fulfills §S without needing it later.

## Divergences from references (deliberate, documented)

- **Health body shape:** `{ok: true, version}` vs opencode's `{healthy: true, version}`. Plan-locked.
- **Session.permission_mode coarse enum** vs opencode's full ruleset. Plan-locked per §A. Per-session ruleset complexity belongs to Phase 04.
- **Async runtime:** end-to-end async per Decisions Log (plan.md §17.5). Revisit if testing pain emerges in Phase 08.

## Open questions

None blocking Phase 01. All concrete enough to implement.

## Files cited

- `temp/opencode/packages/opencode/src/server/server.ts:1-219`
- `temp/opencode/packages/opencode/src/server/routes/instance/httpapi/api.ts:30-52`
- `temp/opencode/packages/opencode/src/session/session.ts:208-228`
- `temp/opencode/packages/opencode/src/cli/cmd/serve.ts:7-24`, `cli/network.ts:5-32`
- `temp/opencode/packages/opencode/src/plugin/index.ts:60-261`
- `temp/opencode/packages/sdk/js/src/gen/types.gen.ts:384-560`
- `temp/claw-code/rust/Cargo.toml:1-22`
- `temp/claw-code/rust/crates/api/src/error.rs:25-360`
- `temp/claw-code/rust/crates/rusty-claude-cli/src/main.rs:200-242,392,4842`
- `temp/claw-code/rust/crates/api/src/client.rs:10-14`, `providers/mod.rs:18`
