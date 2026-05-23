---
phase: 3
title: "Agent Loop Core"
status: pending
priority: P1
effort: "2w"
dependencies: [1, 2]
---

# Phase 3: Agent Loop Core

> **Amendments apply.** See [amendments-after-red-team.md](./amendments-after-red-team.md) §H (doom guard + cost circuit breakers), §T (duplicate tool_call index detection) and [amendments-plugin-system.md](./amendments-plugin-system.md) §6 (Phase 03) — `before_turn`/`after_turn`/`on_chat_*`/`on_cost_tick` hook integration; `quota` plugin owns max_steps + cost limits.

## Overview

Implement the conversation runtime: streaming OpenAI-compat provider, the agent loop, the `Processor` for incremental SSE-frame → Part materialization, doom-loop guard, cost tracking via `rust_decimal`, and `CancellationToken` plumbing. This is the heart of the agent — every later phase plugs into it.

## Requirements

**Functional:**
- `OpenAiCompatProvider` streams chat completions over SSE; supports cancellation, tool calls, usage, finish_reason
- Generic `ConversationRuntime<P,M,A,T,E,Pm>` (monomorphized over adapter generics) drives the loop
- `Processor` consumes raw `ChatDelta` frames and emits `Part` writes + `AgentEvent` publishes; rebuilds tool_call args from streamed `delta.tool_calls[].function.arguments` chunks
- Loop terminates on `finish_reason in {end_turn, max_tokens, error, cancelled}`; continues on `tool_use` after running tools
- Doom-loop guard: reject if last 3 assistant turns produce identical tool_calls (same name + same args hash)
- Cost tracking: per-turn `rust_decimal::Decimal` from `provider.pricing(model)` × usage; recorded on `step_finish` part
- Cancellation: token tree — session token → turn token → tool token; user cancel propagates everywhere
- Reasoning/thinking parts captured if present in stream; routed through projection per phase-02 rules

**Non-functional:**
- Zero `Box<dyn>` on the hot streaming path; provider stream is the trait's associated `Stream` type
- One write transaction per assistant turn
- Frame-to-Part latency < 16ms p99 (measured via `tracing` span)
- Unit-testable without HTTP — `MockProvider` fixture replays canned SSE bytes

## Architecture

**Three-layer split** (mirrors claw-code, NOT opencode):
1. `OpenAiCompatProvider` — pure HTTP streaming. No domain knowledge. Emits raw `ChatDelta`.
2. `Processor` — pure logic. `ChatDelta + ProcessorState -> (Vec<PartWrite>, Vec<AgentEvent>, NextState)`. No IO.
3. `ConversationRuntime` — orchestrator. Owns the cancellation tree, batches DB writes, calls into `ToolExecutor`, decides when to loop.

This split lets phase-08 add a `MockAnthropicService`-style parity harness by swapping the provider only.

**`OpenAiCompatProvider` (`openlet-adapters/src/openai_compat/`):**
- `provider.rs` — public struct + `ModelProvider` impl
- `wire.rs` — `ChatRequest` ↔ OpenAI JSON (request shape)
- `sse.rs` — hand-rolled SSE parser per researcher §4 (mirror `claw-code/rust/crates/api/src/sse.rs`); strips `data:` prefix, handles `[DONE]`, multi-line frames, ping events
- `stream.rs` — `impl Stream<Item = Result<ChatDelta, ProviderError>>` wrapping `reqwest::Response::bytes_stream()` + `AsyncBufReadExt::lines()`
- `pricing.rs` — static table `HashMap<&str, ModelPricing>` for the OpenRouter models we care about; `Decimal` per million tokens
- `error.rs` — `ProviderError` typed: `Network|HttpStatus(u16)|Decode|Cancelled|RateLimited{retry_after}|InvalidApiKey|ContextLengthExceeded|Other(String)`. `safe_failure_class()` returns stable label for metrics (claw-code §4.5 pattern).

**`ChatDelta` shape:**
```rust
pub enum ChatDelta {
    Role(Role),
    Content { text: String },
    Reasoning { text: String, signature: Option<String> },
    ToolCallStart { call_id: String, name: String, index: usize },
    ToolCallArgsDelta { index: usize, args_chunk: String },
    Finish { reason: FinishReason, usage: Option<Usage> },
}
```

**`Processor` state machine** (`openlet-core/src/runtime/processor.rs`):
```rust
pub struct ProcessorState {
    pub current_text: Option<String>,
    pub current_reasoning: Option<String>,
    pub pending_tool_calls: BTreeMap<usize, PendingToolCall>, // by index
    pub usage: Option<Usage>,
    pub finish: Option<FinishReason>,
}
```
On each `ChatDelta`:
- `Content` → buffer into `current_text`; emit `MessagePartUpdated` event (TUI streams this).
- `ToolCallArgsDelta` → append to `pending_tool_calls[index].args_buf`; do NOT parse JSON yet (chunks may not be valid JSON mid-stream).
- `Finish` → flush text/reasoning into Parts, validate every `pending_tool_calls.args_buf` parses as JSON (else error), emit `step_finish` Part with `usage` and computed cost.

**`ConversationRuntime::run_turn` pseudo:**
```rust
async fn run_turn(&self, session: SessionId, cancel: CancellationToken) -> Result<TurnOutcome> {
    let msgs = self.memory.list_messages(session).await?;
    let llm_msgs = project_for_llm(&msgs, ...);
    let req = build_chat_request(&self.agent_def, &llm_msgs, &self.tool_registry);
    let mut stream = self.provider.chat_stream(req, cancel.child_token()).await?;
    let mut state = ProcessorState::default();
    let tx = self.memory.begin_tx().await?;
    while let Some(delta) = stream.next().await {
        let delta = delta?;
        let (writes, events, next) = Processor::step(state, delta);
        for w in writes { self.memory.append_part_tx(&tx, ...).await?; }
        for e in events { self.events.publish(e).await?; }
        state = next;
        tokio::select! { _ = cancel.cancelled() => return Ok(TurnOutcome::Cancelled), else => {} }
    }
    tx.commit().await?;
    Ok(self.classify_outcome(state))
}
```

**Doom-loop guard** (`openlet-core/src/runtime/doom_guard.rs`):
After each assistant turn, hash the set of `(tool_name, args_canonical_json)` from this turn's tool_calls. Compare against the previous 2 turns. If all 3 match, abort with `Error::DoomLoop` and a synthetic assistant message: `"Detected repeated tool calls — aborting to prevent loop. Please refine your request."`. Pattern from opencode `session/index.ts`.

**Cancellation tree:**
- `session_token` lives on `SessionMeta` (created at session start)
- `turn_token = session_token.child_token()` per assistant turn; cancelled by `POST /v1/session/:id/cancel` route
- `tool_token = turn_token.child_token()` per tool call; cancelled when turn cancels OR tool-specific timeout
- `tokio::process::Command::kill_on_drop(true)` on every spawned subprocess

**Cost tracking:**
- `Pricing { input_per_mtok: Decimal, output_per_mtok: Decimal, cache_read_per_mtok: Option<Decimal>, cache_write_per_mtok: Option<Decimal> }`
- `let cost = (usage.prompt as Decimal / 1_000_000) * pricing.input_per_mtok + (usage.completion as Decimal / 1_000_000) * pricing.output_per_mtok;`
- Recorded as string in `step_finish` payload (decimal-as-string, never f64)

## Related Code Files

**Create:**
- `crates/openlet-adapters/src/openai_compat/{provider.rs,wire.rs,sse.rs,stream.rs,pricing.rs,error.rs}`
- `crates/openlet-core/src/runtime/{mod.rs,processor.rs,doom_guard.rs,cost.rs,turn.rs}`
- `crates/openlet-core/src/agent/{mod.rs,definition.rs,registry.rs}` — `AgentDefinition`, `register_agents()` (manual, NO inventory)
- `crates/openlet-core/tests/processor_tests.rs` — table-driven `ChatDelta` → `(writes,events)` cases
- `crates/openlet-adapters/tests/openai_compat_sse_tests.rs` — wiremock-driven streaming fixtures

**Modify:**
- `crates/openlet-core/src/lib.rs` — re-export `runtime`, `agent`
- `crates/openlet-server/src/app_state.rs` — add `Arc<ConversationRuntime<...>>`
- `crates/openlet-adapters/Cargo.toml` — `reqwest` features `["json","rustls-tls","stream"]`, `default-features=false`

**Delete:** none.

## Implementation Steps

1. **Provider scaffold.** Define `ModelProvider::Stream` associated type as `BoxStream<'static, Result<ChatDelta, ProviderError>>` for now (revisit when GATs stabilize cleanly across the runtime). Build `OpenAiCompatProvider::new(base_url, api_key, http: reqwest::Client)`.
2. **Wire types** (`wire.rs`). `ChatRequest` → JSON via `serde`. Tool schema serialized from `schemars::schema_for!(T)` per registered tool input struct (phase-04 supplies these — for phase-03 tests use a dummy `EchoTool`).
3. **SSE parser.** Port `claw-code/rust/crates/api/src/sse.rs` line-by-line. Test against captured fixtures from OpenRouter (commit two: one with tool_calls, one with reasoning). Handle `event: ping` no-ops.
4. **Stream impl.** `reqwest::Response::bytes_stream()` → `tokio_util::io::StreamReader` → `AsyncBufReadExt::lines()` → SSE parser → `ChatDelta` mapper. Cancellation via `tokio::select!` against `cancel.cancelled()`.
5. **Pricing table.** Hardcode 6-8 OpenRouter models (gpt-4o, claude-3.5-sonnet, etc.) with literal `Decimal::from_str("3.00")` values. Document update procedure in module doc — pricing changes are infrequent and a manual PR is cheaper than a config file no one will maintain.
6. **Processor.** Pure functions, no async. `Processor::step(state, delta) -> (writes, events, next_state)`. Drive with table-driven tests covering: pure-text, text+single-tool, text+parallel-tools, reasoning-then-text, mid-stream finish, malformed args JSON.
7. **Doom guard.** Stateless function: `fn check(history: &[Vec<ToolCallSig>]) -> DoomVerdict`. Sig = `(name, blake3(canonical_json(args)))`. Tested in isolation.
8. **Cost calc.** `fn compute_cost(usage: &Usage, pricing: &Pricing) -> Decimal`. Table-driven.
9. **`ConversationRuntime`.** Generic over six adapters via `AppState`'s type params. Owns:
   - tool_registry (phase-04 supplies; phase-03 stubs an empty one + `EchoTool` for tests)
   - cancellation_tokens: `DashMap<SessionId, CancellationToken>` keyed by session
   - runtime metrics: counters for turns, tool_calls, doom_aborts (tracing-only, no Prometheus yet)
10. **`run_loop`.** Outer loop: `while finish_reason == ToolUse { run_turn -> execute_tools -> append tool results }`. Doom-guard check before each iteration. Compaction-as-loop-step is phase-07 — for phase-03 just abort if context overflow signal received from provider.
11. **Agent registry.** `AgentDefinition { id, prompt_segments: (cacheable, dynamic), tool_allowlist, model_id, default_temperature }`. `register_agents() -> Vec<AgentDefinition>` is a plain function in `openlet-core::agent::registry` returning the bundled set (general agent + indexer stub). Server `main.rs` calls it once.
12. **Wire to AppState.** Replace stub `OpenAiCompatProvider` from phase-01 with the real one. Read `OPENROUTER_API_KEY` from env; refuse to start if missing.
13. **Tests.** Two tiers:
    - Unit (pure): processor, doom-guard, cost, projection roundtrip
    - Integration: `wiremock` fixture serves canned SSE bytes; runtime drives a turn; assert Parts written + events published

## Reference Cross-Check (MANDATORY before coding)

Spawn parallel exploration subagents on:
- **opencode**: `packages/opencode/src/session/index.ts` (the giant loop — extract its tool-call-state-machine + doom guard + step boundaries), `packages/opencode/src/provider/anthropic.ts` and `openai.ts` (reasoning vs content separation), `packages/opencode/src/util/cost.ts`.
- **claw-code**: `rust/crates/api/src/sse.rs` (port literally, just rename), `rust/crates/api/src/conversation.rs` (sync ApiClient facade — confirm we're matching its event ordering), `rust/crates/api/src/error.rs::safe_failure_class` (port the function), `rust/crates/runtime/src/runtime.rs` (cancellation tree shape — verify our token hierarchy matches).

Confirm or revise: `ChatDelta` enum granularity, doom-guard threshold (3 vs 5 — claw uses 3), cost calc precision (decimal places), pricing table membership.

## Success Criteria

- [ ] `cargo test -p openlet-core --test processor_tests` — all variants green
- [ ] `cargo test -p openlet-adapters --test openai_compat_sse_tests` — wiremock fixtures replay correctly
- [ ] Manual smoke: `OPENROUTER_API_KEY=... cargo run -p openlet-server`, hit `prompt_async` once via curl with an agent + simple prompt, observe streamed Parts in DB and JSONL log
- [ ] Cancellation smoke: start a long turn, `POST /v1/session/:id/cancel`, observe turn aborts within 200ms
- [ ] Doom-loop test: synthetic provider that returns identical tool_calls — runtime aborts on 3rd turn with `step_finish.reason="error"` and a synthetic assistant message
- [ ] Cost smoke: known fixture with usage `{prompt:1000, completion:500}` and pricing `(3.0, 15.0)` per Mtok → recorded cost `0.0105`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Provider SSE format drift across OpenRouter models | H | M | Hand-rolled parser, tested per-model fixture; capture real bytes when adding new model |
| Tool-args JSON arrives in mid-token chunks | H | M | Buffer until Finish before parsing; surface `Decode` error if invalid at end |
| Cancellation leaks subprocess | M | H | `kill_on_drop(true)` always; integration test that kills mid-bash and asserts `pgrep` clean |
| Doom guard false-positives on legit retry patterns | M | M | Hash includes args; test with identical-tool-different-args case; threshold = 3 not 2 |
| Cost rounding drift (f64 contamination) | L | M | Lint forbids `as f64` in cost.rs; only `Decimal` arithmetic |
| Generic `Runtime<P,M,A,T,E,Pm>` blows compile time | M | L | Confine generics to runtime crate; AppState alias hides them at the route layer |
| Provider 429 → infinite retry | M | H | Single-retry policy with backoff; surface `RateLimited{retry_after}` to TUI |

## Next Steps

Phase 4 plugs `ToolExecutor` impls into the registry and adds permissions. Phase 5 exposes `prompt_async` + cancellation over HTTP and turns event publishes into SSE frames.
