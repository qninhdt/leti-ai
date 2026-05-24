# Custom agents

openlet-ai is plugin-driven: agents are Rust types that implement
`openlet-plugin-api::Plugin` and register one or more `AgentDef`s into
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
use openlet_plugin_api::{Plugin, PluginContext, PluginManifest, PluginError};
use openlet_core::agent::AgentDef;

pub struct ResearchAgentPlugin;

#[async_trait]
impl Plugin for ResearchAgentPlugin {
    fn manifest(&self) -> &PluginManifest {
        // id, semver, capability declarations, default config
        &MANIFEST
    }

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let agent = AgentDef::builder("research")
            .system_prompt(include_str!("prompts/research.md"))
            .allow_tools(["read", "glob", "grep", "bash"])
            .max_tokens(8192)
            .build();
        ctx.register_agent(agent)?;
        Ok(())
    }
}
```

The plugin lives in its own crate under `crates/openlet-plugins/<your-plugin>/`,
gets added to `openlet-plugin-registry::all_plugins()`, and is installed
once during server boot.

See `crates/openlet-plugins/core-agents/` for the shipped reference (a
`general` agent and an `indexer` stub) — start by copying it.

## What `PluginContext` exposes

- `register_agent(AgentDef)` — primary way to introduce new agent types
- `register_tool(ToolSpec, impl ToolHandler)` — add a tool the runtime
  will dispatch to
- `core_api()` — read-only handle to the runtime's adapter facade for
  things like emitting events or reading config

The full surface lives in `openlet-plugin-api/src/context.rs`.

## Workspace and permissions

Each agent gets its own `AgentSpec` with a workspace root. File tools
are scoped to that root (the `LocalFilesystem` adapter rejects paths
outside the workspace; see `crates/openlet-adapters/src/localfs/`).
Permissions are evaluated by `ConfigPermissionMgr` against rulesets
declared in config — the plugin doesn't need to think about it.

## Testing your plugin

The runtime ships an integration-test pattern: spawn the
`openlet-test-mock-provider` server, point a real `OpenAiCompatProvider`
at it, drive a turn, assert events. See
`crates/openlet-adapters/tests/openai_compat_parity.rs` for the
canonical example.

## Cross-checks before shipping a plugin

- `cargo deny check` — your transitive deps must satisfy the
  workspace license + advisory policy (`deny.toml`).
- `cargo clippy --workspace --all-targets -- -D warnings`
- The shipped `safe_failure_class()` taxonomy must cover your error
  variants — extend `FailureClass` if your tool surfaces a new failure
  mode.
