# PM Report — Phase 7 Complete

Plan: `plans/20260523-1414-openlet-agent-core-mvp`
Date: 2026-05-23

## Phase status

| # | Phase | Status |
|---|-------|--------|
| 1 | Foundation | Complete |
| 2 | Storage and Message Model | Complete |
| 3 | Agent Loop Core | Complete |
| 4 | Tools and Permissions | Complete |
| 4D | Filesystem Adapter and Agent Invariant | Pending |
| 5 | HTTP API and SSE | Complete |
| 6 | Ink TUI | Complete |
| 7 | Compaction and Polish | Complete |
| 8 | Hardening | Pending |

7/9 phases complete. Remaining: 4D (filesystem adapter + agent invariant) + 8 (hardening).

## Phase 7 deliverables shipped

- `Part::Compaction` variant + projection substitution rule
- `runtime::compaction` (synthetic-prompt-based summarization-as-loop-step)
- `runtime::token_estimate` (bytes/4 heuristic + provider-actual override seam)
- `agent::{AgentDefinition, AgentRegistry, AgentSlug, PromptSegments}`
- `core-agents` plugin crate (`general` + `indexer`) — first plugin shipping built-in behaviour
- `PluginContext::register_agent` / `take_registered_agents`
- BLAKE3 prompt-cache hash lock (amendment §R)
- Post-compaction overflow check (amendment §P)
- Custom-agent README walkthrough (`crates/openlet-core/src/agent/README.md`)

## Tests

- 131 tests (was 51 before phase 7) green across 34 suites
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- Workspace release build clean

## Code-review verdict

DONE_WITH_CONCERNS → all HIGH+MEDIUM findings landed:
- F-1 (HIGH): synthetic prompt + verbatim summary now included in `compacted_message_ids` so neither leaks into the next projection. Regression test in `tests/projection_compaction.rs`.
- F-2 (MEDIUM): `model_steps` counter replaces `for step in 1..=max_steps`; compaction iterations no longer burn step budget.
- F-3 (LOW): `last_actual_tokens = None` after compaction so the stale anchor doesn't survive.
- New unit test for `superseded_messages` covers off-by-one risk on the `body[..split]` slice.

## Deferred (user-confirmed)

- End-to-end smoke test (wiremock + permission ask + cancel + reconnect) → phase-08, where it pairs with the `MockAnthropicService`-style parity harness already planned.
- `tiktoken-rs` token estimator → phase-08 hardening; current bytes/4 heuristic + provider-actual override is the agreed MVP shape.
- `SessionMeta.agent_slug` column → phase-08 follow-up. MVP route falls back to `general` slug; flagged for a `tracing::warn!` once per session in `routes/message.rs:240-244`.

## Acceptance criteria recap (vs phase-07 doc)

| # | Criterion | Status |
|---|-----------|--------|
| 1 | Token estimator accuracy fixture | Deferred (tiktoken phase-08) |
| 2 | Compaction triggers at 80% threshold | Met (`tests/compaction_trigger.rs`) |
| 3 | `compacted_message_ids` correct + projection substitutes | Met (`tests/projection_compaction.rs` + F-1 regression) |
| 4 | Next prompt drops ≥50% | Implicit in projection test; explicit token diff test deferred to phase-08 |
| 5 | General agent full smoke | Deferred (user) |
| 6 | Indexer "not yet implemented" | Deferred (user) |
| 7 | `register_agents()` lists both | Met (`tests/install_registers_agents.rs`); HTTP route added in phase-08 |
| 8 | End-to-end smoke | Deferred (user) |
| 9 | TUI folded card | Out of scope (`tui/` is separate) |
| 10 | Custom-agent README walkthrough | Met (`agent/README.md`) |
| 11 | Clippy clean | Met |
| 12 | BLAKE3 prompt cache lock | Met |
| 13 | Pruning unit-tested | Partial (added `superseded_messages` test; full per-tool-output pruning test still tied to amendment §D phase-02 deliverable) |
| 14 | Post-compaction overflow check | Met (`error.rs:33`, `turn_loop.rs:148-152`) |

## Risk surface

- F-1 dormant in production (no live session triggers compaction yet). Patch landed before any real session compacts.
- The MVP route always picks the `general` slug; an `indexer` session would compact at general's threshold and use general's prompt. Flagged for phase-08.
- TUI `Part::Compaction` rendering not implemented; `tui/` is untracked. Will need a folded-card view per phase-07 spec.

## Unresolved questions

- Should `PluginContext::register_agent` return `Result<(), RegistryError>` so duplicate-slug-from-same-plugin surfaces at the call site instead of host-drain? (Current behavior: drains then `AgentRegistry::insert` rejects.)
- `tui/` re-generation strategy for new `Part` variants — hand-edit or codegen? Out of scope this phase.
