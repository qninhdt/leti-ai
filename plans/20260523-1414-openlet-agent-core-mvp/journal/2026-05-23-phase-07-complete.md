# Phase 7 Complete — Compaction + core-agents Plugin

Date: 2026-05-23

## What landed

- **Compaction-as-loop-step.** `runtime::compaction` triggers at the top of each `run_loop` iteration when projected tokens cross `agent.context_window * agent.compaction_threshold` (default 0.8). A synthetic user message asks the model to summarize older history; result becomes a `Part::Compaction` that the projection layer substitutes for the listed `compacted_message_ids`.
- **`core-agents` plugin.** First plugin shipping built-in behaviour. Installs the `general` (full tool catalog) and `indexer` (read-only stub) agents through `PluginContext::register_agent`. Drained at boot in `openlet-server::main::install_agents`.
- **`AgentDefinition` + `AgentRegistry`.** Slug-keyed registry; definitions hold split `PromptSegments` (cacheable + dynamic).
- **Prompt-cache hash lock** (amendment §R). BLAKE3 of the `general_cacheable.md` is asserted in `tests/prompt_cache_hash.rs`; silent edits now break CI.
- **Post-compaction overflow check** (amendment §P). New `CoreError::ContextOverflowAfterCompaction` if the summary itself overflows.
- **Custom-agent README** at `crates/openlet-core/src/agent/README.md` — 4-step recipe for adding a third agent.

## What got cut / deferred

User-confirmed up-front:

| Item | Why deferred |
|------|--------------|
| End-to-end smoke (wiremock + permission ask + cancel + reconnect) | Pairs better with phase-08's `MockAnthropicService` parity harness |
| `tiktoken-rs` token estimator | bytes/4 + provider-actual override is the agreed MVP shape; trait shape is stable for upgrade |
| `SessionMeta.agent_slug` column | Phase-08 task; route falls back to `"general"` slug for now |

## Reference cross-check resolved two doc errors

Spawned parallel exploration agents on `temp/opencode` and `temp/claw-code` per the phase doc's MANDATORY directive. Findings:

- **claw-code has NO Hermes timestamp prefix.** The phase-07 doc (line 89, 169) referenced a "Hermes timestamp pattern" attributed to claw-code's `conversation.rs`. Repo-wide grep returned zero hits. The nearest equivalent is `runtime/src/prompt.rs:198-219` injecting a stable date into the system-prompt environment section — once per session, not per turn. Implemented the dynamic segment as `Workspace + Date` per the simpler claw-code reality.
- **claw-code has NO `AgentDefinition`.** No agent abstraction at all. Design came entirely from opencode's `agent.ts:29-50` (Schema.Struct shape), Rust-ified.

opencode's compaction pattern (`session/overflow.ts:6-32` + `session/compaction.ts`) was the actual reference for the trigger. Their threshold is `context - max_output_tokens` rather than a percentage; we kept the percentage form per the phase-07 spec because openlet-ai doesn't yet model `max_output_tokens` per provider.

## Code-review HIGH bug fixed pre-finalize

Reviewer caught **F-1 (HIGH)**: the synthetic compaction-request user message AND the verbatim summary text from the compaction turn were both persisted but NOT included in `compacted_message_ids`, so they leaked into every subsequent projection — model would see a stray "Summarize the conversation history above" and the summary twice.

Fix at `runtime/turn_loop.rs:104-130`: append `synth_id` and `outcome.assistant_message_id` to the superseded list before writing the `Part::Compaction`. Regression test in `tests/projection_compaction.rs` asserts neither leak survives.

Also fixed F-2 (compaction iterations consuming `max_steps` budget; replaced `for step in 1..=max_steps` with explicit `model_steps` counter) and F-3 (stale `last_actual_tokens` after compaction).

## What broke during implementation

- **`Part::Compaction` blast radius.** Adding the variant tripped `non_exhaustive_patterns` errors at three callers: `openlet-protocol/src/dto/part.rs` (DTO `From` impl), `openlet-adapters/src/sqlite/memory_store.rs` (`part_kind` for the `kind` column), and the `id()` accessor itself. Each caught by `cargo check --workspace` immediately and fixed with a single arm.
- **Plugin amendment §6 vs phase-07 doc body conflict.** Doc body says "manual `register_agents()`, NO inventory"; amendment overrides to "agents ship via `core-agents` plugin." Asked the user; chose full plugin path. Required new `openlet-plugins/core-agents` crate, `PluginContext::register_agent`, and the boot-time install drain. Adds ~400 LoC of plumbing but matches the cloud-customization north star.
- **`SessionMeta` doesn't carry `agent_slug` yet.** Plan amendment §6 implied the slug would be plumbed end-to-end; phase-02 schema only has `agent_id` (UUID). MVP route at `routes/message.rs:240-244` hardcodes the `"general"` lookup. Documented as a phase-08 follow-up rather than retrofitting phase-02 schema mid-phase.

## Numbers

- 131 tests passing across 34 suites (was 51 / 7 before phase 7 → +80 / +27)
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- Workspace release build clean
- 7/9 plan phases complete (phases 4D + 8 remaining)

## Open questions

- Should `PluginContext::register_agent` return `Result` so duplicate-slug-from-same-plugin surfaces at the call site rather than at host-drain? Currently fine but bug is one rename away.
- TUI `Part::Compaction` rendering — `tui/` is untracked and out-of-tree. Folded-card view per phase-07 spec is a phase-08 task whoever owns the TUI picks up.
