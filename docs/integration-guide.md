# Integration Guide

Embedding `openlet-server` as a library inside a downstream binary
(e.g. Openlet Cloud). The runnable `openlet-server` binary is the local
mode; the cloud binary is a **superset** that imports the same crates,
swaps adapters, layers auth, and adds routes — without forking core.

## Library boundary

| Layer | Crate | Role |
|---|---|---|
| Runtime | `openlet-core` | Conversation loop, tool dispatcher, traits |
| Adapters | `openlet-adapters` | Reference impls (sqlite, localfs, OpenAI-compat, …) |
| Plugin SPI | `openlet-plugin-api` | `Plugin` trait, `PluginContext`, `CoreApi` |
| Plugin host | `openlet-plugin-registry` | Plugin discovery + drain |
| HTTP/SSE | `openlet-server` | `AppState`, `AppStateBuilder`, `RouterBuilder`, route handlers |

**Core sees only `AgentId`.** The user→agent map lives entirely upstream
in the integrator. `SessionMeta` does not carry `user_id` (phase 4 adds
an opaque `extensions: serde_json::Value` slot for that).

## Canonical embedding

```rust
use std::sync::Arc;
use openlet_server::{AppStateBuilder, RouterBuilder, routes};
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
    //    Plugins land via PluginRegistry once phase 3 wires register_tool /
    //    register_provider / hooks. For now, .plugin_registry(_) carries
    //    the registry into AppState; agents drain via install_agents.
    let state = AppStateBuilder::new()
        .provider(provider)
        .memory(memory)
        .permission(permission)
        .events(events)
        .artifacts(artifacts)
        .tools(tools)
        .tool_registry(tool_registry)
        .config(Arc::new(config.clone()))
        .agents(agents)
        .default_agent_id(default_agent_id)
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
| `AppState` | `openlet_server` | The shared handle every route reads from |
| `AppStateBuilder` | `openlet_server` | Fluent constructor with required-field validation |
| `AppStateBuilderError` | `openlet_server` | Error returned by `.build()` when a required field is missing |
| `RouterBuilder` | `openlet_server` | Fluent router composer; pick which route groups to mount |
| `AgentResources` | `openlet_server` | Per-agent `(spec, fs, shell)` triple stored in `AppState::agents` |
| `routes::*` | `openlet_server` | Each handler is `pub async fn` — re-mount one at a time when overriding |
| `Plugin`, `PluginContext`, `PluginManifest` | `openlet_plugin_api` | Author plugins for tools/agents/hooks |
| `CoreApi` | `openlet_plugin_api` | Back-channel for plugins (read-only into core) — body lands in phase 4 |
| `MemoryStore`, `EventSink`, `ModelProvider`, `PermissionManager`, `ArtifactStore`, `Filesystem`, `ToolExecutor` | `openlet_core::adapters` | Six-trait adapter split — swap any backend |
| `EventSink::subscribe(EventFilter)` | `openlet_core::adapters` | Out-of-band event tap for billing/audit/SIEM |
| `POST /v1/session/:id/abort` | HTTP | Cancels the active turn — already cascades into the running loop |

## What integrators control vs. what core controls

| Concern | Owner | Notes |
|---|---|---|
| Auth (login, sessions, JWT, OAuth) | Integrator | Layer middleware over `RouterBuilder::build` |
| User → agent ownership | Integrator | Track in own DB; pass `AgentId` to core via `CreateSessionDto.agent_id` |
| Quota / billing | Integrator | Subscribe to `EventSink` or use `before_turn` hook (phase 3) + `cancel_session` (phase 5) |
| Audit logging | Integrator | `EventSink::subscribe(EventFilter::all)` |
| Permission policy | Integrator | Supply own `Arc<dyn PermissionManager>` |
| Custom tools / agents / providers | Integrator | Plugin via `PluginContext::register_*`. Even the eight built-in tools (`read`, `list`, `glob`, `grep`, `write`, `edit`, `bash`, `todo`) ship through the `core-tools` plugin — proof the surface is sufficient |
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
use openlet_server::routes::session;
let app = Router::new()
    .route("/v1/session", post(cloud::create_session_with_ownership_check))
    .route("/v1/session", get(session::list))
    .route("/v1/session/:id", get(session::get_one).delete(session::delete))
    .route("/v1/session/:id/mode", post(session::set_mode))
    .route("/v1/session/:id/abort", post(crate::routes::cancel::abort))
    .merge(core_router)
    .with_state(state);
```

## Local binary backward compat

The reference binary in `crates/openlet-server/src/main.rs` uses
`AppStateBuilder` + `RouterBuilder::default()`. Behavior is identical to
the previous monolithic `build_router(state)` call. `build_router(state)`
is kept as a thin wrapper around `RouterBuilder::default().build(state)`
so existing tests + downstream callers don't churn.

## Versioning

`openlet-plugin-api` declares `core_version_req` on every manifest. Core
bumps the major when the extension API breaks. Pin your plugin's
`core_version_req` to the version range you tested against.

## Next steps

- Phase 3 wires the 13 plugin hooks (`before_turn`, `on_chat_params`, …)
  and adds `register_tool` / `register_provider` to `PluginContext`.
- Phase 4 adds `extensions: serde_json::Value` to `SessionMeta` so
  integrators bind `user_id` without forking core types.
- Phase 5 adds `CoreApi::cancel_session` so plugins can stop a session
  mid-flight from any hook.
- Phase 6 ships the `core-tools` plugin: the 8 built-in tools register
  through `PluginContext::register_tool` like any custom tool, proving
  the public surface is sufficient.
- Phase 7 ships the `tests/integration_smoke.rs` regression gate that
  asserts (a) `install_all` drains all 8 built-in tools + the `general`
  agent through the public surface, (b) the `test-quota-stub` plugin
  installs its hook chain cleanly, and (c) `extensions["user_id"]`
  round-trips through SQLite. Clone it as a starting reference for
  downstream integrators. The runnable `examples/integration-shape/`
  reference crate is deferred until a concrete downstream integrator
  (Cloud) lands and informs its shape.

## Workspace routing (multi-tenant)

Cloud deployments serve multiple workspaces from a single binary. The
`openlet-server::workspace_resolver` + `middleware::workspace_routing`
modules make this routing pluggable.

### Trait surface

```rust
use openlet_server::workspace_resolver::{WorkspaceResolver, WorkspaceError};
use std::sync::Arc;

#[async_trait::async_trait]
impl WorkspaceResolver for cloud::CloudResolver {
    async fn resolve(
        &self,
        workspace_id: &str,
    ) -> Result<Arc<openlet_server::AppState>, WorkspaceError> {
        // Look up the workspace's BYOK keys + plugin set, build a per-
        // workspace AppState, and cache it. Cache invalidation is the
        // integrator's responsibility — when the control plane mutates
        // a workspace, evict its cached entry.
        self.cache.get_or_build(workspace_id).await
    }
}
```

### Mount order (CRITICAL)

```text
auth middleware → WorkspaceRoutingLayer → handler
```

`WorkspaceRoutingLayer` and the `WorkspaceRoutingGuard` extractor BOTH
refuse to proceed unless an `AuthPrincipal` is already in the request
extensions. Mounting workspace routing before auth produces 401 on
every request — loud-fail by design, because skipping auth before
workspace lookup is a cross-tenant data exposure.

```rust
let resolver = cloud::CloudResolver::new(control_plane);
let app = Router::new()
    .route("/v1/turn", post(handler))
    .layer(openlet_server::WorkspaceRoutingLayer::new(resolver))
    .layer(cloud::AuthLayer::new(jwt_validator));  // mounts FIRST → runs FIRST
```

The HTTP header `x-openlet-workspace` selects the workspace. Header
parsing is lenient (missing → falls back to `default`) so single-tenant
deployments work with the included `StaticWorkspaceResolver`.

### Per-workspace data root

```rust
use openlet_server::workspace_data_root;

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

openlet-ai is zero-trust: identity is never read from an upstream-injected
header — it must come from a verified credential. The `openlet_server::auth`
module ships two pluggable traits, each with a local default, plus the
canonical identity types.

### Inbound: `Authenticator`

```rust
use openlet_server::{AuthError, AuthPrincipal, Authenticator};

#[async_trait::async_trait]
impl Authenticator for cloud::JwksAuthenticator {
    async fn authenticate(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<AuthPrincipal, AuthError> {
        // RS256-verify the bearer token against openlet's JWKS, reject
        // act-bearing tokens, map the verified subject → AuthPrincipal.
        let token = bearer(headers).ok_or(AuthError::MissingCredential)?;
        let claims = self.verify(token).map_err(|e| AuthError::InvalidCredential(e.to_string()))?;
        Ok(AuthPrincipal { caller_id: claims.sub, principal_type: PrincipalType::User })
    }

    fn is_dev(&self) -> bool { false } // not the admit-all dev default
}
```

Mount it via `RouterBuilder::build_with_auth(state, Arc::new(my_auth))`. The
default `build(state)` mounts `LocalDevAuthenticator` (admits one fixed
principal, no token) — correct for `./openlet-ai` loopback dev, refused on a
non-loopback bind and under `OPENLET_RUNTIME_PROFILE=cloud` (fail-closed).
The `AuthLayer` runs before the workspace layer and injects the
`AuthPrincipal` the workspace gate + question route require.

### Runtime profile

`OPENLET_RUNTIME_PROFILE=local|cloud` (default `local`). `local` resolves the
dev authenticator; `cloud` makes `authenticator_for_profile` fail closed —
the cloud binary MUST build its own `Authenticator` and call
`build_with_auth` rather than relying on the default.

### Outbound: `CredentialProvider`

When an agent tool calls an openlet service, it carries openlet-ai's own
service-account credential scoped to the agent workspace (external SA, not a
per-turn user token). The trait + a `NoopCredentialProvider` default (returns
`Ok(None)`) live in `openlet_server::auth`; the holder is on
`AppState::credential_provider` (set via `AppStateBuilder::credential_provider`).

```rust
use openlet_server::{CredentialProvider, OutboundCredential, AgentWorkspace};

#[async_trait::async_trait]
impl CredentialProvider for cloud::SaIssuer {
    async fn workspace_credential(
        &self,
        ws: &AgentWorkspace,
    ) -> Result<Option<OutboundCredential>, openlet_server::CredentialError> {
        Ok(Some(self.mint_sa_token(ws).await?))
    }
}
```

**Threading status:** the seam is built and held, but NOT yet wired into any
tool call — core has no outbound HTTP tool today (every builtin is local:
bash/edit/read/write/glob/grep/list/todo/ask_user/plan_mode/subagent_task/
task_status). **Plug-in point:** when a real outbound tool lands, it reads
`AppState::credential_provider`, calls `workspace_credential(agent_workspace)`,
and attaches the returned bearer as `Authorization: Bearer <token>` on its
outbound request. Until then the noop default keeps the local binary
credential-free.
