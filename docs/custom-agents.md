# Custom agents

leti-ai is plugin-driven: agents are Rust types that implement
`leti-plugin-api::Plugin` and register one or more `AgentDefinition`s into
the runtime's `AgentRegistry` at boot.

## When to add a custom agent

- You want a specialized prompt + tool surface (e.g. an indexer that only
  has `read`, `glob`, `grep`).
- You want a different default model or token budget for a subset of
  tasks.
- You want a tool that doesn't ship with core (a custom HTTP client, a
  domain-specific compiler).

If all you want is a tweaked system prompt, prefer adding a project-level
prompt override; reach for a plugin once you need code.

## Anatomy of a plugin

```rust
use async_trait::async_trait;
use leti_plugin_api::{Plugin, PluginContext, PluginManifest, PluginError};
use std::sync::Arc;
use leti_core::agent::{AgentDefinition, AgentSlug, PromptSegments};

pub struct ResearchAgentPlugin;

#[async_trait]
impl Plugin for ResearchAgentPlugin {
    fn manifest(&self) -> &PluginManifest {
        // id, semver, capability declarations, default config
        &MANIFEST
    }

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let agent = AgentDefinition {
            slug: AgentSlug::new("research").expect("static slug"),
            title: "Research".into(),
            description: "Read-focused research agent.".into(),
            prompt_segments: Some(PromptSegments {
                cacheable: include_str!("prompts/research.md").into(),
                dynamic: Arc::new(|_| String::new()),
            }),
            tool_allowlist: ["read", "glob", "grep", "bash"]
                .into_iter().map(str::to_owned).collect(),
            model_id: None,
            default_temperature: 0.0,
            context_window: 8192,
            compaction_threshold: 0.8,
            compaction_summary_cap_tokens: 2_000,
            hidden: false,
        };
        agent.validate().map_err(PluginError::InvalidConfig)?;
        ctx.register_agent(agent)?;
        Ok(())
    }
}
```

The plugin lives in its own crate under `crates/leti-plugins/<your-plugin>/`,
gets added to `leti-plugin-registry::all_plugins()`, and is installed
once during server boot.

See `crates/leti-plugins/core-agents/` for the shipped reference (a
`general` agent and an `indexer` stub) — start by copying it.

## What `PluginContext` exposes

- `register_agent(AgentDefinition)` — primary way to introduce new agent types
- `register_tool(ToolSpec, impl ToolHandler)` — add a tool the runtime
  will dispatch to
- `core_api()` — read-only handle to the runtime's adapter facade for
  things like emitting events or reading config

The full surface lives in `leti-plugin-api/src/context.rs`.

## Workspace and permissions

Each agent gets its own `AgentSpec` with a workspace root. File tools
are scoped to that root (the `LocalFilesystem` adapter rejects paths
outside the workspace; see `crates/leti-adapters/src/localfs/`).
Permissions are evaluated by `ConfigPermissionMgr` against rulesets
declared in config — the plugin doesn't need to think about it.

## Host context and interaction mode

The engine has no user, tenant, or principal model. A host that needs request
metadata creates a typed `leti_core::runtime::TurnExtensions` value and puts
it on the turn context. The same opaque carrier reaches `ToolCtx` and
`PermissionCtx`, including child turns. Core never interprets or persists
those values; host-owned filesystem and permission adapters may downcast their
own types.

Sessions are `Interactive` by default. A host may explicitly create a
`Detached { on_ask: Allow|Deny }` session for unattended work. Detached mode
does not override explicit Deny rules, does not blanket-allow destructive
shell operations or `web_fetch`, and emits a durable authorization event for
every detached permission check. Background-injected turns remain fail-closed.

## Testing your plugin

The runtime ships an integration-test pattern: spawn the
`leti-test-mock-provider` server, point a real `OpenAiCompatProvider`
at it, drive a turn, assert events. See
`crates/leti-adapters/tests/openai_compat_parity.rs` for the
canonical example.

## Cross-checks before shipping a plugin

- `cargo deny check` — your transitive deps must satisfy the
  workspace license + advisory policy (`deny.toml`).
- `cargo clippy --workspace --all-targets -- -D warnings`
- The shipped `safe_failure_class()` taxonomy must cover your error
  variants — extend `FailureClass` if your tool surfaces a new failure
  mode.
