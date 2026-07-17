# Integration Guide

Embedding `leti-server` as a library inside a downstream binary
(e.g. Leti Cloud). The runnable `leti-server` binary is the local
mode; the cloud binary is a **superset** that imports the same crates,
swaps adapters, layers auth, and adds routes — without forking core.

## Library boundary

| Layer | Crate | Role |
|---|---|---|
| Runtime | `leti-core` | Conversation loop, tool dispatcher, traits |
| Adapters | `leti-adapters` | Reference impls (sqlite, localfs, OpenAI-compat, …) |
| Plugin SPI | `leti-plugin-api` | `Plugin` trait, `PluginContext`, `CoreApi` |
| Plugin host | `leti-plugin-registry` | Plugin discovery + drain |
| HTTP/SSE | `leti-server` | `AppState`, `AppStateBuilder`, `RouterBuilder`, route handlers |

`leti-core` has no user, tenant, principal, or client-type model. Host-owned
request metadata travels through the typed, opaque `TurnExtensions` carrier;
the engine passes it to permission checks, tools, and subagents but never
interprets or persists it. See [`cloud-integration.md`](cloud-integration.md).

## Canonical embedding

```rust
use std::sync::Arc;
use leti_server::{AppStateBuilder, RouterBuilder, routes};
use axum::Router;
use axum::routing::post;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Boot the integrator's adapters.
    let memory = Arc::new(cloud::PgMemoryStore::new(pg_pool).await?);
    let permission = Arc::new(cloud::Rbac::new(rbac_db));
    let provider = Arc::new(cloud::HeaderInjectingProvider::wrap(
        OpenAiCompatProvider::new(base, api_key),
    ));

    // 2. Build AppState with the integrator's adapter mix.
    //    Plugins land via PluginHandles; agents drain via install_all into
    //    InstalledPlugins, which is plumbed into AppState.plugin_registry.
    let state = AppStateBuilder::new()
        .provider(provider)
        .memory(memory)
        .permission(permission)
        .events(events)
        .artifacts(artifacts)
        .tool_registry(tool_registry)
        .config(Arc::new(config.clone()))
        .agents(agents)
        .default_agent_id(default_agent_id)
        .workspace_root(workspace_root)
        .build()?;

    // 3. Compose the router. Skip core's session::create — the
    //    integrator owns the ownership-checked version. Re-mount the
    //    other session handlers manually after merging the core router.
    let core_router = RouterBuilder::new()
        .with_health_routes()
        .with_message_routes()
        .with_event_routes()
        .with_permission_routes()
        .with_agent_routes()
        .with_plugin_routes()
        .build(state.clone());

    let app = Router::new()
        .route("/v1/session", post(cloud::create_session_with_ownership_check))
        .merge(core_router)
        .merge(cloud::user_routes())
        .layer(cloud::auth_middleware());

    axum::serve(listener, app).await?;
    Ok(())
}
```

## Public extension API

These are the types integrators import. Everything else is internal.

| Type | Crate | Purpose |
|---|---|---|
| `AppState` | `leti_server` | The shared handle every route reads from |
| `AppStateBuilder` | `leti_server` | Fluent constructor with required-field validation |
| `AppStateBuilderError` | `leti_server` | Error returned by `.build()` when a required field is missing |
| `RouterBuilder` | `leti_server` | Fluent router composer; pick which route groups to mount |
| `AgentResources` | `leti_server` | Per-agent `(spec, fs, shell)` triple stored in `AppState::agents` |
| `routes::*` | `leti_server` | Each handler is `pub async fn` — re-mount one at a time when overriding |
| `Plugin`, `PluginContext`, `PluginManifest` | `leti_plugin_api` | Author plugins for tools/agents/hooks |
| `CoreApi` | `leti_plugin_api` | Read-only runtime back-channel for plugins |
| `MemoryStore`, `EventSink`, `ModelProvider`, `PermissionManager`, `ArtifactStore`, `Filesystem` | `leti_core::adapters` | Six-trait adapter split — swap any backend |
| `EventSink::subscribe(EventFilter)` | `leti_core::adapters` | Out-of-band event tap for billing/audit/SIEM |
| `POST /v1/session/:id/abort` | HTTP | Cancels the active turn — already cascades into the running loop |

## What integrators control vs. what core controls

| Concern | Owner | Notes |
|---|---|---|
| Auth (login, sessions, JWT, OAuth) | Integrator | Layer middleware over `RouterBuilder::build` |
| User → agent ownership | Integrator | Track in own DB; pass `AgentId` to core via `CreateSessionDto.agent_id` |
| Quota / billing | Integrator | Subscribe to `EventSink` or use hooks + `cancel_session` |
| Audit logging | Integrator | `EventSink::subscribe(EventFilter::all)` |
| Permission policy | Integrator | Supply own `Arc<dyn PermissionManager>` |
| Custom tools / agents / providers | Integrator | Plugin via `PluginContext::register_*`. Core tools, including `read`, `list`, `glob`, `grep`, `write`, `edit`, `bash`, `todo`, and opt-in `web_fetch`, ship through the `core-tools` plugin — proof the surface is sufficient. `web_fetch` needs a host-injected `WebFetcher` and should remain Ask-by-default. |
| Conversation loop, tool dispatch, projection | Core | Don't fork; extend via plugins |
| Cancellation cascade | Core | `POST /v1/session/:id/abort` cancels the live turn |
| Cost calc, persistence, SSE | Core | Adapter-swappable but algorithm stays in core |

## Selective router composition

`RouterBuilder::default()` mounts everything (matches the local binary).
For a cloud binary that overrides one route, use `RouterBuilder::new()`
and skip the group containing the route to override:

```rust
// Skip session-routes — integrator mounts a custom session::create that
// checks ownership before delegating into the runtime. They re-mount the
// other session handlers manually.
let core_router = RouterBuilder::new()
    .with_health_routes()
    .with_message_routes()
    .with_event_routes()
    .with_permission_routes()
    .with_agent_routes()
    .with_plugin_routes()
    .build(state.clone());

// Mount the integrator's session::create + core's other session routes
// at the merge boundary.
use leti_server::routes::session;
let app = Router::new()
    .route("/v1/session", post(cloud::create_session_with_ownership_check))
    .route("/v1/session", get(session::list))
    .route("/v1/session/:id", get(session::get_one).delete(session::delete))
    .route("/v1/session/:id/mode", post(session::set_mode))
    .route("/v1/session/:id/abort", post(routes::cancel::abort))
    .merge(core_router)
    .with_state(state);
```

## Local binary backward compat

The reference binary in `crates/leti-server/src/main.rs` uses
`AppStateBuilder` + `RouterBuilder::default()`. Behavior is identical to
the previous monolithic `build_router(state)` call. `build_router(state)`
is kept as a thin wrapper around `RouterBuilder::default().build(state)`
so existing tests + downstream callers don't churn.

## Versioning

`leti-plugin-api` declares `core_version_req` on every manifest. Core
bumps the major when the extension API breaks. Pin your plugin's
`core_version_req` to the version range you tested against.

## Next steps

The reference `core-tools` and `core-agents` plugins demonstrate the public
registration surface. The `test-quota-stub` crate demonstrates cost hooks;
clone it as a starting point for a host billing plugin.

## Workspace routing (multi-tenant)

Cloud deployments serve multiple workspaces from a single binary. The
`leti-server::workspace_resolver` + `middleware::workspace_routing`
modules make this routing pluggable.

### Trait surface

```rust
use leti_server::workspace_resolver::{WorkspaceResolver, WorkspaceError};
use std::sync::Arc;

#[async_trait::async_trait]
impl WorkspaceResolver for cloud::CloudResolver {
    async fn resolve(
        &self,
        principal: &leti_server::AuthPrincipal,
        workspace_id: &str,
    ) -> Result<Arc<leti_server::AppState>, WorkspaceError> {
        // Look up the workspace's BYOK keys + plugin set, build a per-
        // workspace AppState, and cache it. Cache invalidation is the
        // integrator's responsibility — when the control plane mutates
        // a workspace, evict its cached entry.
        self.cache.get_or_build(principal, workspace_id).await
    }
}
```

### Mount order (CRITICAL)

```text
auth middleware → WorkspaceRoutingLayer → handler
```

The host must authenticate before workspace lookup and must authorize the
requested workspace before returning an `AppState`. The reference server's
`WorkspaceRoutingLayer` and `LocalDevAuthenticator` are local conveniences;
a cloud host supplies its own verifier/resolver or wraps the base router with
its middleware. Mounting workspace routing before authentication must fail
closed to avoid cross-tenant lookup.

```rust
let resolver = cloud::CloudResolver::new(control_plane);
let app = Router::new()
    .route("/v1/turn", post(handler))
    .layer(leti_server::WorkspaceRoutingLayer::new(resolver))
    .layer(cloud::AuthLayer::new(jwt_validator));  // mounts FIRST → runs FIRST
```

The HTTP header `x-leti-workspace` selects the workspace. Header
parsing is lenient (missing → falls back to `default`) so single-tenant
deployments work with the included `StaticWorkspaceResolver`.

### Per-workspace data root

```rust
use leti_server::workspace_data_root;

let ws_root = workspace_data_root(&data_dir, workspace_id)?;
// → {data_dir}/workspaces/{ws_id}/   (path-traversal-safe)
let db_path = ws_root.join("db.sqlite");
```

`workspace_data_root` rejects ids containing `/`, `\`, `..`, NUL, or
control characters. Each workspace's SQLite + artifact root lives in a
distinct subdirectory so a path-traversal bug in id parsing cannot leak
data across tenants.

See `docs/multi-provider.md` for the per-workspace BYOK pattern with
`MultiProvider`.

## Inbound auth & outbound credentials

Authentication and credential issuance are host responsibilities. The
reference `leti-server` exposes a pluggable `Authenticator` only for its
loopback-oriented HTTP composition; a cloud deployment must provide its own
verified middleware and authorization layer. Do not add auth or tenant
semantics to `leti-core`. If a port needs request-scoped credential data, pass
an opaque host-defined value through `TurnExtensions` as described in
[`cloud-integration.md`](cloud-integration.md).

## Adapter contract spec (cloud impl conformance)

Each adapter trait was widened so a cloud backend is expressible without
forking core. A cloud impl must satisfy the behaviors below; the local
impls' existing test suites are the executable reference (no generic test
kit ships — run your own targeted tests against these behaviors).

### MemoryStore — pagination + scoping
- `list_sessions_paged` / `list_messages_paged` return at most `Page::limit`
  rows plus a `next_cursor`; `None` cursor means the last page. The cursor
  is **opaque** — callers pass it back verbatim, never parse it.
- A paged walk must cover the same set, in the same order, as the
  unbounded `list_*` (reference: `sqlite_memory_store` paged tests).
- Soft-delete: `delete_session` sets `deleted_at`; default `list_*`
  excludes deleted unless `SessionFilter::include_deleted`.
- Cross-workspace isolation: a cloud store keys rows by workspace and MUST
  NOT return another tenant's sessions. `SessionId` is globally unique, so
  no per-method partition arg is needed — scope at the connection/store
  level. Assert cross-workspace reads return empty/NotFound.

### ArtifactStore — streaming + presign
- `get_stream` must reassemble to the same bytes as buffered `get`
  (reference: `localfs_artifact_store` streaming round-trip).
- `put_stream` collects/streams a body equivalent to `put`.
- `presign(ref, op)` returns `Some(url)` only if the backend supports
  direct client transfer (S3/MinIO); local returns `None`. A `Get` URL
  must download the same bytes; a `Put` URL must accept an upload that a
  later `get` reads back.

### EventSink — routing + delivery
- `publish_routed` with a `RoutingKey` delivers to subscribers scoped to
  that workspace/user; the local bus ignores the key (broadcasts to all)
  via the default. Durable events still carry a monotonic `event_id` in
  assignment = persist = broadcast order (the replay-seam contract).
- `delivery_semantics()` truthfully reports `BestEffort` (frames may drop
  to a slow subscriber) or `AtLeastOnce` (durable, consumer dedupes on
  `event_id`). Transient (`part.delta`/`heartbeat`) events are never
  persisted regardless.

### ModelProvider / WorkspaceResolver / PermissionManager
- Already impl-agnostic — see the per-trait audit notes in source. A
  remote provider, control-plane resolver, or cloud authz service plugs in
  with no signature change. `WorkspaceResolver::resolve` receives the
  authenticated principal and returns `Forbidden` on an ownership
  mismatch.

## Quota / cost-cap integration (plugin seam, not a trait)

There is **no `Quota` trait**. Cost control is the plugin cost-tick seam,
demonstrated by `test-quota-stub` (`crates/leti-plugins/test-quota-stub`):

- `on_cost_tick`: consult the host's billing context, decrement the balance
  by `delta_usd`, and call `CoreApi::cancel_session` when it hits zero — the
  active turn unwinds mid-flight.
- `before_turn`: re-check the balance and return `HookResult::Stop` so the
  next turn never starts (covers the cancel-vs-next-iteration race).

Cost numbers come from core's verified cost path; the plugin only decides
when to stop. Leti Cloud forks this plugin to call its billing/ledger
service instead of the in-memory map. **Fail-closed vs fail-open on a
ledger outage is the integrator's choice** — local stays fail-open (no cap,
logs); a cloud deploy should deny on ledger-unreachable.

**Open question (owner):** does leti-ai own a self-contained cost ledger,
or call a future leti quota service? Determines whether the cost-tick
plugin is storage-backed or a remote client. No leti LLM-cost service
exists today, so this stays a plugin seam with the decision deferred.
