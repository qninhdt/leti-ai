---
phase: 7
title: "Compaction and Polish"
status: complete
priority: P2
effort: "1.5w"
dependencies: [1, 2, 3, 4, 5, 6]
---

# Phase 7: Compaction and Polish

> **Amendments apply.** See [amendments-after-red-team.md](./amendments-after-red-team.md) §D (pruning test), §P (post-compaction overflow check), §R (prompt cache hash lock) and [amendments-plugin-system.md](./amendments-plugin-system.md) §6 (Phase 07) — rework: general agent + indexer ship via `core-agents` plugin (NOT a hardcoded `register_agents()`); compaction trigger fires `on_compaction` hook chain.

## Overview

Layer compaction onto the existing turn loop (compaction-as-loop-step), wire the general agent end-to-end, ship the indexer reference custom agent (stub-grade — proves the registration flow), and run the first full smoke test pass. By end of this phase, an MVP user can hold long conversations without context overflow and developers can see exactly how a custom agent is defined.

## Requirements

**Functional:**
- Compaction triggers when projected token count > `agent.context_window * 0.8`
- Compaction strategy: synthetic user message asking the model to summarize the oldest N% of the conversation, summary saved as `Part::Compaction`
- After compaction, projection (phase-02 already supports this) substitutes summary in place of compacted messages
- Compaction is a NORMAL turn — runs through the same `run_loop`, hits the same provider, billed normally
- General agent fully wired with prompt segments, tool allowlist, default model
- Indexer reference custom agent registered + invokable; performs only stub work (logs "indexing started", returns "not yet implemented")
- Custom-agent docs in `crates/openlet-core/src/agent/README.md` showing how to add a new agent in code
- End-to-end smoke test: scripted conversation hitting both general + indexer agent, with tool calls + permission ask + cancel + reconnect

**Non-functional:**
- Compaction never compacts the most recent N=4 messages (preserves immediate context)
- Compaction never runs concurrently with another turn on the same session (serialized via session token)
- Token estimation accuracy ≥ 90% vs actual provider count (use `tiktoken-rs` or provider-supplied token count)
- Compaction summary capped at 2 KB tokens

## Architecture

**Compaction as a loop step.** The runtime's `run_loop` already alternates between LLM turns and tool execution. Compaction inserts a NEW kind of "turn" preceded by a synthetic user message. Pseudo:

```rust
loop {
    if context_pressure(session) > 0.8 {
        // 1. Insert synthetic user message asking for summary
        let synth = Message::synthetic_user("Summarize the conversation so far. Focus on...");
        memory.append_message(synth).await?;
        // 2. Run a normal turn against a NARROWER projection (system + last 4 + recent tool outputs)
        let outcome = self.run_turn_for_compaction(session).await?;
        // 3. Save assistant text as Part::Compaction with compacted_message_ids list
        memory.append_part(Part::Compaction { summary, compacted_message_ids, ... }).await?;
        // 4. Continue normal loop — projection now sees the summary in place of old messages
    }
    let outcome = self.run_turn(session).await?;
    if outcome.finish == ToolUse { execute_tools(); continue; }
    break;
}
```

**Why this works.** Phase-02's `project_for_llm` already substitutes Compaction parts in place of `compacted_message_ids`. Phase-07 just adds the trigger + the synthetic-user-message dance.

**Trigger heuristic:**
- Maintain a running estimate via `tiktoken-rs` (free, no network)
- For OpenRouter Anthropic models, prefer the provider-supplied `usage.prompt_tokens` from the previous turn — more accurate than tiktoken
- Threshold: 80% of `agent.context_window`. Configurable per agent.

**Synthetic user prompt** (compaction request):
```
Summarize the conversation history above. Preserve:
- The user's overall goal
- Key decisions and constraints established
- Files read or modified (paths only)
- Tool errors encountered and resolutions
Drop:
- Verbose tool output bodies
- Code snippets superseded by later edits
- Idle chatter
Output format: bullet points under headers (Goal, Decisions, Files, Errors).
Limit: 500 words.
```

**General agent definition** (`openlet-core/src/agent/builtin/general.rs`):
```rust
pub fn general_agent() -> AgentDefinition {
    AgentDefinition {
        id: "general".into(),
        title: "General Assistant".into(),
        prompt_segments: PromptSegments {
            cacheable: include_str!("./general_cacheable.md").to_string(),
            dynamic: dynamic_segment_fn,  // includes timestamp prefix per claw-code Hermes pattern
        },
        tool_allowlist: vec!["read","list","glob","grep","write","edit","bash","todo"],
        model_id: "anthropic/claude-3.5-sonnet".into(),
        default_temperature: 0.0,
        context_window: 200_000,
        compaction_threshold: 0.8,
    }
}
```

**Indexer reference custom agent** (`openlet-core/src/agent/builtin/indexer.rs`):
```rust
pub fn indexer_agent() -> AgentDefinition {
    AgentDefinition {
        id: "indexer".into(),
        title: "Workspace Indexer (stub)".into(),
        prompt_segments: PromptSegments {
            cacheable: "You are a workspace indexer. For MVP this agent only logs and returns 'not yet implemented'.".into(),
            dynamic: |_| String::new(),
        },
        tool_allowlist: vec!["read","list","glob"],   // read-only
        model_id: "anthropic/claude-3.5-haiku".into(),
        default_temperature: 0.0,
        context_window: 200_000,
        compaction_threshold: 0.8,
    }
}
```

The indexer is intentionally minimal — its role in MVP is to prove the registration mechanism works, NOT to actually index. Real indexing is post-MVP per the brainstorm.

**`register_agents()`** (`openlet-core/src/agent/registry.rs`):
```rust
pub fn register_agents() -> Vec<AgentDefinition> {
    vec![
        builtin::general::general_agent(),
        builtin::indexer::indexer_agent(),
    ]
}
```

**End-to-end smoke test** (`tests/end_to_end_smoke.rs` in workspace root or `crates/openlet-server/tests/`):
1. Start server in-process with mock provider returning canned SSE
2. Create session with `general` agent
3. Send prompt that triggers a `read` tool call
4. Assert tool result appears as `Part::ToolCall(state=Completed)`
5. Send prompt that triggers a `bash` tool call (rule says `ask`)
6. Assert SSE emits `PermissionRequested`
7. POST permission reply with `allow`
8. Assert tool runs and result appears
9. Cancel mid-turn; assert state transitions
10. Reconnect SSE with `Last-Event-ID`; assert no missed events
11. Switch to `indexer` agent in a new session; send prompt; assert response includes "not yet implemented"

## Related Code Files

**Create:**
- `crates/openlet-core/src/runtime/compaction.rs` — trigger logic + synthetic message + compaction-turn orchestration
- `crates/openlet-core/src/runtime/token_estimate.rs` — `tiktoken-rs`-backed estimator + provider-actual override
- `crates/openlet-core/src/agent/builtin/{mod.rs,general.rs,general_cacheable.md,indexer.rs}`
- `crates/openlet-core/src/agent/README.md` — how to define a custom agent
- `crates/openlet-server/tests/end_to_end_smoke.rs`

**Modify:**
- `crates/openlet-core/src/runtime/turn.rs` — call `compaction::maybe_compact` at top of each iteration
- `crates/openlet-core/src/agent/registry.rs` — wire `register_agents()`
- `crates/openlet-server/src/main.rs` — call `register_agents()` and store in AppState
- `crates/openlet-core/Cargo.toml` — add `tiktoken-rs`

**Delete:** none.

## Implementation Steps

1. **Token estimator.** `estimate_tokens(messages: &[LlmMessage]) -> usize` using `tiktoken-rs` `cl100k_base` encoder (good-enough across providers). Override path: if last `step_finish.usage.prompt_tokens` is present for this session, use that as anchor + estimate only the delta since.
2. **Trigger.** `should_compact(session, agent) -> bool`: `estimate_tokens > agent.context_window * agent.compaction_threshold`.
3. **Synthetic user message.** `Message::synthetic_user(content)` constructor with a flag `synthetic: true` in meta JSON so TUI can render it differently (greyed out, prefix "[auto]"). It IS persisted — auditability matters.
4. **Compaction turn.** `run_turn_for_compaction` is `run_turn` with two changes:
   - Projection narrows: keeps system + most recent 4 messages + the synthetic prompt, summarizes everything else
   - On finish, the assistant's text is wrapped into `Part::Compaction { summary, compacted_message_ids, original_token_count, ... }` rather than `Part::Text`
5. **Loop integration.** At the top of each `run_loop` iteration, before `run_turn`, check `should_compact`. If true, run compaction turn first; loop. Compaction turn itself MUST NOT recurse into another compaction (use a flag on the turn context).
6. **General agent prompt.** Two segments: cacheable (mission, tone, tool catalog with descriptions, safety rules) and dynamic (timestamp prefix per Hermes pattern, current workspace dir, current date). Cacheable segment placed FIRST so Anthropic prompt cache hits.
7. **Indexer agent.** Minimal definition above.
8. **Custom-agent README.** Walkthrough: define `AgentDefinition`, add to `register_agents()`, rebuild server. Cite the indexer as the canonical example.
9. **End-to-end smoke.** Use `wiremock` for the provider. Drive the full flow. Run as `cargo test -p openlet-server --test end_to_end_smoke`.
10. **TUI tweak.** TUI shows compaction summaries with a distinct visual (folded card, "Compacted N messages" header, expandable). Phase-06 already has `compaction` in the Part union — extend renderer.

## Reference Cross-Check (MANDATORY before coding)

Spawn parallel exploration subagents on:
- **opencode**: `packages/opencode/src/session/compact.ts` (their compaction trigger + prompt — port the prompt verbatim if applicable), `packages/opencode/src/agent/agent.ts` (AgentDefinition shape — confirm fields).
- **claw-code**: `rust/crates/api/src/conversation.rs` (Hermes timestamp prefix pattern), `rust/crates/runtime/src/agent.rs` if present (custom agent registration patterns).

Confirm or revise: compaction prompt wording, threshold (80% vs 70%), preserve-recent count (4 vs 6), token estimator choice (tiktoken-rs vs provider-actual primary).

## Success Criteria

- [ ] Token estimator within 10% of provider-actual on a 10-turn fixture
- [ ] Compaction triggers at 80% threshold; verified by synthetic large conversation
- [ ] Compaction summary has correct `compacted_message_ids` list; projection substitutes correctly
- [ ] After compaction, next turn's prompt token count drops by ≥ 50%
- [ ] General agent: full smoke (prompt → read → write → step_finish with cost) works
- [ ] Indexer agent: invocation returns "not yet implemented" text without errors
- [ ] `register_agents()` lists both agents on `GET /v1/agent`
- [ ] End-to-end smoke test passes
- [ ] TUI renders compaction summary as folded card; expanding reveals full summary
- [ ] Custom-agent README walkthrough produces a working third agent in <10 min (manual test)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Compaction summary loses critical context | M | H | Prompt explicitly preserves goal/decisions/files; preserve-recent buffer of 4 messages; user can disable per-agent |
| Compaction recurses (compaction triggers compaction) | L | H | Hard flag on turn context blocks re-trigger; assertion in tests |
| Token estimator wildly wrong → premature/late compaction | M | M | Prefer provider-actual when available; alarm if estimator vs actual diverges > 20% |
| Indexer stub confuses users | L | L | README clearly marks "stub"; indexer's first response always says so |
| Compaction race with cancel | M | M | Compaction runs under turn token; cancel cancels both |
| `tiktoken-rs` adds heavy dep | L | L | Pure-Rust, BPE only, ~1MB; acceptable |
| Custom-agent footgun: forgotten allowlist letting agent run dangerous tools | M | H | Default allowlist is empty; README emphasizes explicit opt-in; CI lint flags empty allowlist as warning |

## Next Steps

Phase 8 hardens the system: clippy/fmt clean, README, integration suite per adapter, mock-anthropic-style parity harness, distribution prep.
