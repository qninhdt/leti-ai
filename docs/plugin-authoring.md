# Plugin Authoring Guide

This guide documents the surface every plugin uses: the `Plugin` trait,
manifests, capability declarations, the `PluginContext` registration API,
the 14 hook signatures, and three worked examples (quota enforcement,
audit logging, request-header injection).

A plugin is a single `impl Plugin` registered through `openlet-plugin-registry`.
At server boot the host calls `install`, drains the context into a sorted
`HookChains`, and forwards every dispatch site through the merged chain.
Plugins author *only* against `openlet_plugin_api::prelude` — the rest of
the workspace is private surface.

---

## 1. The `Plugin` trait

```rust
use async_trait::async_trait;
use openlet_plugin_api::prelude::*;

#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    fn manifest(&self) -> &PluginManifest;
    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError>;
    async fn shutdown(&self) -> Result<(), PluginError> { Ok(()) }
}
```

`install` is the single registration window — register agents, tools,
provider, hooks, then return. The host drains the context, sorts the
chains canonically, and freezes them for the lifetime of the process.
`shutdown` is best-effort; do not rely on it for correctness.

`PluginError` variants:

- `Install { id, message }` — fatal during install; aborts the chain.
- `IncompatibleCoreVersion { id, req, have }` — `core_version_req`
  unsatisfied; the host filters before calling `install`.
- `InvalidConfig(String)` — `ctx.config::<T>()` failed.
- `Runtime(String)` — capability gate or registration violation.

---

## 2. `PluginManifest`

```rust
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: Version,
    pub description: String,
    pub author: Option<String>,
    pub capabilities: Vec<Capability>,
    pub core_version_req: VersionReq,
    pub default_priority: u8,
    pub config_schema: Option<schemars::schema::Schema>,
}
```

Required fields:

- `id` — stable identifier; collisions across plugins are rejected at
  load time. Used as `manifest_id` for hook ordering tie-breaks.
- `version` — semver; `core_version_req` semver-matches the host's core
  version. Plugins that don't match are skipped with a structured log.
- `capabilities` — closed set the plugin will exercise. Hooks/tools/
  providers register only if the matching capability is declared
  (host emits `PluginError::Runtime` otherwise).
- `default_priority` — used when a hook registers without an explicit
  `Priority`. Range 0–255. Higher runs earlier.

Optional `config_schema` powers the host's per-plugin config validation
ahead of `install`.

---

## 3. `Capability`

```rust
pub enum Capability {
    Tool,
    Agent,
    Provider,
    Hook(HookKind),
    Permission,
    Telemetry,
    Storage,
}
```

The capability gate is enforced by `PluginContext`: registering a hook
without `Capability::Hook(kind)` (matching the exact `HookKind`)
returns `PluginError::Runtime`. Same rule for `register_tool`,
`register_provider`, `register_agent`. Declaring more than needed is
fine; declaring less is a fatal install error.

---

## 4. `PluginContext` — registration API

`install(&self, ctx)` receives a fresh `PluginContext`. Methods:

| Method | Purpose |
|---|---|
| `ctx.manifest()` | Read-only manifest reference. |
| `ctx.config::<T>()` | Deserialize the per-plugin config block. |
| `ctx.core()` | `Arc<dyn CoreApi>` back-channel into the host. |
| `ctx.register_agent(def)` | Register an `AgentDefinition`. |
| `ctx.register_tool(handle)` | Register a `ToolHandle`. Needs `Capability::Tool`. |
| `ctx.register_provider(provider)` | Register a `ModelProvider`. First-wins across plugins; needs `Capability::Provider`. |
| `ctx.on_<hook>(priority, func)` | Register a typed hook handler. Needs `Capability::Hook(<kind>)`. |

The eight built-in tools (`read`, `list`, `glob`, `grep`, `write`,
`edit`, `bash`, `todo`) ship through this surface — see
`crates/openlet-plugins/core-tools/src/lib.rs` for the canonical
`register_tool` example. If MVP can dogfood its own tools through the
plugin API, downstream integrators can register custom tools the same
way without forking core.

Hooks are pushed in registration order; the host calls `chains.sort_all()`
once after every plugin's `install` completes. Sort key:

```
priority desc → manifest_id asc → registration_index asc
```

Ties between plugins resolve alphabetically by `manifest_id`, ties
within a plugin resolve by registration order.

---

## 5. Hook signatures

All 14 hooks share the same shape: an immutable handler that receives
a typed context and returns `HookResult<Ctx>`. Handlers are `async`,
isolated against panics, and bounded by a 5s per-hook timeout.

```rust
pub enum HookResult<I> {
    Continue(I),                                  // pass mutated ctx forward
    Replace(I),                                   // pass mutated ctx forward + audit log
    Stop(I),                                      // halt chain, keep terminal value
    Deny { reason: String, feedback: Option<String> },
}
```

`Continue` and `Replace` are behaviorally identical — `Replace` adds a
structured audit log entry. `Stop` halts subsequent hooks but the
runtime treats the carried value as terminal. `Deny` short-circuits and
the runtime decides per dispatch site whether to surface the reason as
a synthetic tool result (loops continue) or halt the turn (provider
calls).

The 14 hook kinds and where they fire:

| Hook | Ctx | Site |
|---|---|---|
| `OnChatParams` | model / max_tokens / temperature | `ConversationRuntime::run_turn` before request build |
| `OnChatMessages` | model / system_prompt / messages | `ConversationRuntime::run_turn` after params |
| `OnChatHeaders` | model / headers | `ConversationRuntime::run_turn` after messages |
| `BeforeTurn` | session / turn_index / message_count | top of `run_loop` iteration |
| `AfterTurn` | session / turn_index / finish_reason / usage / cost | after each turn |
| `OnStepFinish` | session / step_index / finish_reason / usage | after `AfterTurn` |
| `OnCostTick` | session / model / delta_usd / total_usd / usage | end of every provider call |
| `BeforeToolCall` | invocation + permission ctx | before each tool dispatch |
| `AfterToolCall` | invocation + outcome | after each tool dispatch |
| `OnPermissionAsk` | tool / scope / mode | when permission manager prompts |
| `OnMessage` | session / message | after `MemoryStore::append_message` |
| `OnSessionStatus` | session / status | after `MemoryStore::update_status` |
| `OnCompaction` | session / phase / message_count | wraps compaction in `run_loop` |
| `OnEvent` | event | every `EventSink::publish` (firehose) |

`OnEvent` is treated specially: `Stop`/`Deny` are downgraded so the
sink still forwards the (possibly-mutated) event to downstream
observers. Every other hook follows the standard four-outcome contract.

---

## 6. Failure isolation

Every hook runs under three guards:

1. **Construction panic** — `panic!` inside the closure body before the
   future is returned is caught via `std::panic::catch_unwind`.
2. **Polling panic** — `panic!` while awaiting the returned future is
   caught via `FutureExt::catch_unwind`.
3. **Timeout** — `tokio::time::timeout` with a 5s ceiling.

All three surface as `DispatchOutcome::Denied` carrying a `PluginFault`
with `plugin_id`, `hook`, and a closed `FaultKind` taxonomy
(`ConstructionPanic` / `PollPanic` / `Timeout`). Runtime sites publish
that as `AgentEvent::PluginError` so cloud operators can grep
`event.kind = plugin_error` without parsing log strings.

---

## 7. Example 1 — quota enforcement

Halts a session when cumulative cost crosses a configured ceiling.
`OnCostTick` sees every provider call, so it's the right place to
enforce the budget.

```rust
use openlet_plugin_api::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct QuotaCfg { ceiling_usd: rust_decimal::Decimal }

pub struct Quota;

#[async_trait::async_trait]
impl Plugin for Quota {
    fn manifest(&self) -> &PluginManifest { &MANIFEST }

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let cfg: QuotaCfg = ctx.config()?;
        let ceiling = cfg.ceiling_usd;
        ctx.on_cost_tick(Priority(80), move |c: OnCostTickCtx| {
            let ceiling = ceiling;
            async move {
                if c.total_usd >= ceiling {
                    HookResult::Stop(c)        // halts loop with finish=Halted
                } else {
                    HookResult::Continue(c)
                }
            }
        })?;
        Ok(())
    }
}
```

Manifest declares `Capability::Hook(HookKind::OnCostTick)` so the
registration is allowed. `Stop` from `OnCostTick` is special-cased by
the runtime: the current turn returns `FinishReason::Halted`, the loop
exits, no further provider calls fire on that session.

---

## 8. Example 2 — audit logging

Persists every assistant message to an external sink without altering
the conversation. Pure observation; `Continue` threads the ctx forward
unchanged.

```rust
use openlet_plugin_api::prelude::*;

pub struct Audit { sink: std::sync::Arc<dyn AuditSink> }

#[async_trait::async_trait]
impl Plugin for Audit {
    fn manifest(&self) -> &PluginManifest { &MANIFEST }

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let sink = self.sink.clone();
        ctx.on_message(Priority(50), move |c: OnMessageCtx| {
            let sink = sink.clone();
            async move {
                if let Some(m) = c.message.as_ref() {
                    sink.record(m.session_id, m.id).await;
                }
                HookResult::Continue(c)
            }
        })?;
        Ok(())
    }
}

trait AuditSink: Send + Sync + 'static {
    fn record(
        &self,
        session: openlet_plugin_api::SessionId,
        message: openlet_plugin_api::MessageId,
    ) -> futures::future::BoxFuture<'static, ()>;
}
```

`OnMessage` fires *after* the message is durable in the memory store, so
the audit sink never logs a record that doesn't exist in `MemoryStore`.
A panic inside `sink.record` is isolated as a `DispatchOutcome::Denied`
with `FaultKind::PollPanic` — the underlying `append_message` already
succeeded so storage stays consistent.

---

## 9. Example 3 — request-header injection

> **Phase-3 limitation.** The provider trait does not yet consume
> headers. `OnChatHeaders` runs and the chain is fault-published, but
> any `Replace` mutation is silently dropped on the way to the
> provider. Phase 4 widens `ModelProvider::chat_stream` so the mutation
> takes effect; this example reads as the future contract.

Adds tracing headers to every provider call. `OnChatHeaders` is a
mutating hook: returning `Replace` (or `Continue`) threads the new
header list to the next plugin.

```rust
use openlet_plugin_api::prelude::*;

pub struct TraceHeaders;

#[async_trait::async_trait]
impl Plugin for TraceHeaders {
    fn manifest(&self) -> &PluginManifest { &MANIFEST }

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        ctx.on_chat_headers(Priority(70), |mut c: OnChatHeadersCtx| async move {
            c.headers.push((
                "x-openlet-trace".into(),
                uuid::Uuid::new_v4().to_string(),
            ));
            HookResult::Replace(c)             // audit-logged mutation
        })?;
        Ok(())
    }
}
```

Multiple plugins registering `OnChatHeaders` compose: each receives the
header list as mutated by the previous plugin, ordered by priority desc
then `manifest_id` asc. `Deny` from any one of them halts the turn with
`FinishReason::Halted` and emits a structured warning.

---

## 10. Session extensions

`SessionMeta` carries an opaque `extensions: serde_json::Value` field
the integrator owns. Core stays auth-blind — no `user_id` /
`tenant_id` field on the typed surface. Plugins read whatever shape the
integrator persisted at create time (or mutated since via
`MemoryStore::update_session_extensions`).

Set it from the HTTP boundary by including `extensions` in
`POST /v1/session`:

```json
{
  "agent_id": "...",
  "extensions": {
    "user_id": "u_123",
    "tenant_id": "t_42",
    "scopes": ["read", "write"]
  }
}
```

`GET /v1/session/:id` echoes the same blob back. Default is `null`
when the integrator omits the field — the on-disk default and the
in-memory default match, so omission round-trips cleanly. The SQLite
column has a `json_valid` CHECK so malformed JSON never lands.

Inside a hook, plugins reach the blob through `CoreApi`:

```rust
ctx.on_cost_tick(Priority::default(), {
    let core = ctx.core();
    move |cost: OnCostTickCtx| {
        let core = core.clone();
        async move {
            if let Some(sid) = cost.session_id {
                if let Some(meta) = core.current_session_meta(sid).await {
                    if let Some(user_id) = meta
                        .extensions
                        .get("user_id")
                        .and_then(serde_json::Value::as_str)
                    {
                        tracing::info!(
                            user_id,
                            total_usd = %cost.total_usd,
                            "billing tick"
                        );
                    }
                }
            }
            HookResult::Continue(cost)
        }
    }
})?;
```

`current_session_meta` is the only async method on `CoreApi` —
`session_cost`, `record_cost`, `emit_event`, and `read_config` are all
synchronous. Keep schemas small: the field round-trips through JSON
text in SQLite and ships across every `SessionDto`.

`emit_event` publishes through the inner sink, *bypassing* the
`OnEvent` plugin chain. This avoids re-entrancy when one plugin's
event would re-trigger another's `OnEvent` handler. If your plugin
needs to observe its own emissions, log directly.

`read_config` exposes a phase-4 whitelist (`default_model`,
`max_cost_per_session_usd`, `bind_addr`); unknown keys return
`Err(...)` so quota gates don't silently observe `Null`. The signature
is `Result<serde_json::Value, String>` (not `Result<T, ConfigError>`)
because trait-method generics break object safety.

---

## 11. Example 4 — per-user quota with mid-flight cancel

Section 7 stops the loop when a *session* exceeds a flat ceiling. A
real cloud integrator wants per-*user* enforcement: each `user_id`
maps to a remaining-credit balance, and a session must abort the
moment its user's balance drains, even mid-flight. The
`test-quota-stub` plugin (in `crates/openlet-plugins/test-quota-stub/`)
shows the canonical pattern. Two hooks compose:

- `on_cost_tick`: reads `extensions["user_id"]`, decrements the
  per-user balance by `delta_usd`, calls `CoreApi::cancel_session`
  and returns `Stop` if the running tick drove balance ≤ 0.
- `before_turn`: re-checks the balance and returns `Stop` if already
  exhausted, so the next turn never issues a model call (no charge
  for a turn the runtime would have cancelled anyway).

Skeleton (full source: `crates/openlet-plugins/test-quota-stub/src/lib.rs`):

```rust
async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
    let cfg: QuotaConfig = ctx.config().unwrap_or_default();
    let balances: Arc<Mutex<HashMap<String, Decimal>>> =
        Arc::new(Mutex::new(cfg.budgets));

    let core = ctx.core();
    let bal = balances.clone();
    ctx.on_cost_tick(Priority::default(), move |c: OnCostTickCtx| {
        let core = core.clone();
        let bal = bal.clone();
        async move {
            let Some(sid) = c.session_id else { return HookResult::Continue(c) };
            let Some(user) = core
                .current_session_meta(sid).await
                .and_then(|m| m.extensions.get("user_id")
                    .and_then(serde_json::Value::as_str).map(str::to_owned))
            else { return HookResult::Continue(c) };

            let exhausted = {
                let mut m = bal.lock().unwrap();
                let e = m.entry(user).or_insert(Decimal::ZERO);
                if let Some(d) = c.delta_usd { *e -= d; }
                *e <= Decimal::ZERO
            };
            if exhausted {
                core.cancel_session(sid, "budget_exhausted".into()).await;
                HookResult::Stop(c)
            } else {
                HookResult::Continue(c)
            }
        }
    })?;
    // before_turn: same balance lookup, Stop if ≤ 0. See full source.
    Ok(())
}
```

Why both hooks? `on_cost_tick` is post-turn — it reacts after the
expensive provider call. `before_turn` catches the *next* turn before
a second model call lands. Without the pre-check, an integrator
forking the plugin and dropping `before_turn` leaks one extra turn
per cancellation.

Manifest must declare both capabilities:

```rust
capabilities: vec![
    Capability::Hook(HookKind::OnCostTick),
    Capability::Hook(HookKind::BeforeTurn),
],
```

`CoreApi::cancel_session` trips the per-session token, marks the
session `Cancelling`, and emits `AgentEvent::SessionStatus`. The
running turn unwinds with `CoreError::Cancelled`; subsequent loop
iterations short-circuit. Idempotent — a second call is a no-op.

---

## 12. Pitfalls

- **Don't block in hooks.** The 5s timeout fires per-hook; long-running
  work (HTTP calls, DB writes) must respect it or the chain halts and
  the runtime emits `AgentEvent::PluginError`. Spawn detached work
  through `tokio::spawn` if observation can run out-of-band.
- **Don't capture mutable shared state without `Arc<Mutex<…>>`.** Hook
  closures are `Send + Sync + 'static`; ordinary `&mut` captures don't
  type-check. Prefer immutable captures + interior mutability.
- **Don't return `Replace` for observation hooks.** Both write a log
  line; reserve `Replace` for genuinely mutating hooks
  (`OnChatParams`, `OnChatMessages`, `OnChatHeaders`) so audit rows
  remain meaningful.
- **Don't panic on user input.** Construction panics surface as
  `Denied { plugin_fault: …ConstructionPanic }` — they halt the chain
  for the rest of the dispatch site. Validate config in `install`,
  return `PluginError::InvalidConfig`.
