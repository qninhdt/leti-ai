# Cloud integration contract

This document is the hand-off boundary for a host embedding leti-ai. The
host depends on the leti crates as a library, supplies cloud adapters at its
composition root, merges its own routes and middleware around the base
`RouterBuilder`, and registers its assistant as a plugin.

## Ownership boundary

The engine owns the conversation loop, tool dispatch, streaming, compaction,
durable events, and the port traits. The host owns authentication, tenant and
workspace authorization, billing, assistant policy, and cloud service
clients. `leti-core` must remain free of HTTP, database, filesystem, network,
cloud, and identity implementations.

There is no shipped cloud filesystem adapter. Implement the `Filesystem` port
from scratch in the host and wire it through `AgentResources`. The same rule
applies to remote memory, artifact, event, model, and permission services.

## Composition root

At boot, construct an `AppState` with host implementations of the relevant
ports:

```rust
let state = AppStateBuilder::new()
    .provider(Arc::new(cloud::ModelProvider::new(...)))
    .memory(Arc::new(cloud::MemoryStore::new(...)))
    .artifacts(Arc::new(cloud::ArtifactStore::new(...)))
    .events(Arc::new(cloud::EventSink::new(...)))
    .permission(Arc::new(cloud::PermissionManager::new(...)))
    .tool_registry(tool_registry)
    .config(Arc::new(config))
    .agents(agents) // HashMap<AgentId, AgentResources>
    .default_agent_id(default_agent_id)
    .workspace_root(workspace_root)
    .build()?;
```

`Filesystem` and `ShellExecutor` are per-agent resources, not
`AppStateBuilder` fields. Build each `AgentResources { spec, fs, shell }` with
the host's implementations before placing it in `agents`.

Use `RouterBuilder` to select the base route groups, then merge host routes
and install authentication/workspace middleware. If the host replaces
session creation, it must authorize ownership before setting
`interaction_mode` or mounting the mode-change route. The engine's mode is a
runtime mechanism, not an authorization decision.

## Opaque per-turn context

Host adapters sometimes need verified request metadata such as a tenant key
or credential handle. Put a host-defined, typed value into
`leti_core::runtime::TurnExtensions` at turn start:

```rust
#[derive(Clone)]
struct CloudRequestContext {
    tenant_id: String,
    credential: Arc<CredentialHandle>,
}

let ext = TurnExtensions::default().with(CloudRequestContext { tenant_id, credential });
```

The carrier reaches `LoopContext`, `PermissionCtx`, `ToolCtx`, and child
subagent turns. A host-owned adapter can call `ctx.ext.get::<CloudRequestContext>()`.
The engine never interprets, persists, serializes, or logs the value. Do not
put identity policy or tenant lookup logic in `leti-core`.

The reference server initializes `LoopContext.ext` to `Default::default()`.
A host that needs this data must own or wrap turn startup and stamp its verified
context before invoking the loop; merely using `RouterBuilder` does not infer
request identity into a turn.

## Assistant plugin

Register the cloud assistant as a normal plugin:

- register an `AgentDefinition` with its prompt segments and allowed tools;
- register host-specific tools and hook handlers;
- keep assistant prompts and business policy in the host plugin;
- use `CoreApi` and `EventSink` for runtime integration rather than reaching
  into private core modules.

The reference `crates/leti-plugins/core-agents` crate is the minimal buildable
example.

## Interaction and detached policy

New sessions default to `Interactive`. The host may accept an explicit
`interaction_mode` on session creation, but authentication and authorization
for that choice belong to host middleware. `Detached { on_ask: Allow|Deny }`
means:

- ordinary Ask decisions resolve without a live human;
- explicit permission Deny always wins;
- `web_fetch` and destructive shell subjects are not blanket-upgraded by
  `on_ask=Allow`;
- background- or sibling-injected turns remain fail-closed;
- every detached permission check emits a durable
  `permission.detached_authorized` audit event, including Danger-mode direct
  allows.

Top-level detached turns are not automatically re-driven after process
restart. A host that needs that behavior must implement an explicit recovery
protocol with idempotency and side-effect reconciliation; SSE replay alone is
not recovery.

## Adapter expectations

Cloud implementations should preserve the local adapter contracts and test
against the corresponding integration suites:

- `MemoryStore`: opaque pagination cursors, soft-delete behavior, and tenant
  isolation;
- `ArtifactStore`: streaming round trips and optional presigned transfers;
- `EventSink`: durable ordering, routing, and truthful delivery semantics;
- `PermissionManager`: explicit Deny precedence, deferred Ask support, and
  durable audit integration;
- `Filesystem`: workspace jail, bounded reads, and no credential logging;
- `ModelProvider`: streamed output, retry semantics, and host credential
  injection without exposing secrets to transcript parts.

The host should add contract tests for authorization, tenant isolation,
credential rotation, detached audit delivery, and retry/idempotency behavior.
