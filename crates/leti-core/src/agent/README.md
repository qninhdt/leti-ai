# Custom Agents

This crate ships the built-in agents (`general`, `indexer`) that Leti
boots with. It's also the canonical example for adding your own agent.

## Add a new agent in 4 steps

1. **Create a definition function** that returns an `AgentDefinition`:

   ```rust
   use std::sync::Arc;
   use leti_core::agent::{AgentDefinition, AgentSlug, PromptSegments};

   pub fn my_agent() -> AgentDefinition {
       AgentDefinition {
           slug: AgentSlug::new("my-agent").unwrap(),
           title: "My Agent".into(),
           description: "What it does in one sentence.".into(),
           prompt_segments: Some(PromptSegments {
               cacheable: include_str!("my_cacheable.md").to_owned(),
               dynamic: Arc::new(|_| String::new()),
           }),
           tool_allowlist: vec!["read".into(), "list".into()],
           model_id: "anthropic/claude-3.5-haiku".into(),
           default_temperature: 0.0,
           context_window: 200_000,
           compaction_threshold: 0.8,
           compaction_summary_cap_tokens: 2_000,
           hidden: false,
           max_cost_per_session_usd: None,
       }
   }
   ```

2. **Write the cacheable prompt** at `my_cacheable.md`. Lead with a
   `# version: 1` header so the prompt-cache hash lock test surfaces
   intentional vs. accidental drift.

3. **Register from a plugin** by calling `ctx.register_agent(my_agent())`
   inside your `Plugin::install`. Plugins themselves are added to the
   compile-time list in `crates/leti-plugin-registry/src/lib.rs`.

4. **Lock the prompt cache** with a BLAKE3 hash test (see
   `tests/prompt_cache_hash.rs` for the pattern). Bumping the prompt
   body without bumping the version + hash will invalidate the
   Anthropic prompt cache for every active session — the test makes
   this a deliberate change rather than a silent footgun.

## Tool allowlist

The allowlist is the *first* gate; permission rules in
`ConfigPermissionMgr` are the second. An agent that lists `bash` in its
allowlist still has to satisfy any `bash:*` rule defined for the
workspace + session.

Default to the smallest set that lets the agent do its job. Empty
allowlists are flagged by lint as a likely mistake.

## Compaction tuning

`compaction_threshold = 0.8` triggers compaction at 80% of the agent's
`context_window`. Drop it to `0.7` for chatty agents, raise to `0.9` for
agents that prefer keeping more raw history. The `compaction_summary_cap_tokens`
caps the summary itself (default 2 KB tokens, ≈ 8 KB chars).

## Models

The `model_id` follows the OpenRouter slug format (`provider/model`).
Override at session creation by passing a `model` override on the
`POST /v1/session/:id/prompt_async` payload — useful for cheap-first /
expensive-fallback routing without redefining the agent.
