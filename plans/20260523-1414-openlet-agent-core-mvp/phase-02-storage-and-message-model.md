---
phase: 2
title: "Storage and Message Model"
status: complete
priority: P1
effort: "1.5w"
dependencies: [1]
---

# Phase 2: Storage and Message Model

> **Amendments apply.** See [amendments-after-red-team.md](./amendments-after-red-team.md) §A (schema columns), §D (tool-output pruning), §F (`session_reads` table), §M (regex secret redaction).

## Overview

Stand up the persistent layer: SQLite via sqlx, embedded migrations, the part-based message model, JSONL session log mirror (claw-code parity), local filesystem artifact store, and a deterministic LLM-message projection that compaction will later prune. Storage is correct in MVP if it survives a process restart and lets the TUI replay any session.

## Requirements

**Functional:**
- `sqlx::migrate!("./migrations")` runs at boot against `~/.openlet/db.sqlite` (path configurable)
- Six tables: `sessions`, `messages`, `parts`, `artifacts`, `events`, `permission_decisions`
- `SqliteMemoryStore` implements `MemoryStore` end-to-end (create/append/list)
- Part variants: `Text`, `Reasoning`, `ToolCall { state: Pending|Running|Completed|Errored }`, `StepFinish`, `Compaction`, `Image`, `File`
- `LocalFsArtifactStore` reads/writes under `~/.openlet/artifacts/<session_id>/<key>` with key sanitization
- LLM-message projection: deterministic function `Vec<Message> -> Vec<openai_compat::Message>` collapsing parts into role-tagged content
- Per-session JSONL mirror at `~/.openlet/sessions/<session_id>.jsonl` (one line per event for replay/audit)
- `cargo sqlx prepare --workspace` produces a committable `.sqlx/` cache (offline mode safe in CI)

**Non-functional:**
- Migrations idempotent; `cargo run` twice does not double-apply
- All writes within a single `prompt_async` turn happen in one `BEGIN ... COMMIT`
- JSONL writes use `O_APPEND` + line-buffered flush; secret redaction on `Authorization`/`api_key` keys
- Storage layer fully testable without HTTP — phase-02 tests use temp dirs only

## Architecture

**Two-layer storage model.** SQLite is the source of truth for *queryable* state; JSONL is the append-only log for *replay/audit*. Both are written together inside the same async block but neither blocks the other on failure (JSONL write errors are logged + counter-incremented but do NOT abort the SQLite commit). Mirrors claw-code's session log pattern (`rust/crates/runtime/src/session_log.rs`) while keeping opencode's queryable shape.

**Schema (migrations/0001_init.sql):**
```sql
CREATE TABLE sessions (
  id            TEXT PRIMARY KEY,         -- uuid v7 (lexicographic order)
  agent_id      TEXT NOT NULL,
  title         TEXT,
  workspace_dir TEXT NOT NULL,
  created_at    INTEGER NOT NULL,         -- ms epoch
  updated_at    INTEGER NOT NULL,
  status        TEXT NOT NULL CHECK(status IN ('idle','running','errored','cancelled'))
);

CREATE TABLE messages (
  id          TEXT PRIMARY KEY,           -- uuid v7
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  role        TEXT NOT NULL CHECK(role IN ('system','user','assistant','tool')),
  seq         INTEGER NOT NULL,           -- monotonic per session
  created_at  INTEGER NOT NULL,
  meta        TEXT NOT NULL DEFAULT '{}'  -- json (model, finish_reason, usage)
);
CREATE INDEX idx_messages_session_seq ON messages(session_id, seq);

CREATE TABLE parts (
  id          TEXT PRIMARY KEY,           -- uuid v7
  message_id  TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  seq         INTEGER NOT NULL,
  kind        TEXT NOT NULL,              -- text|reasoning|tool_call|step_finish|compaction|image|file
  payload     TEXT NOT NULL               -- json blob, schema per kind
);
CREATE INDEX idx_parts_message_seq ON parts(message_id, seq);

CREATE TABLE artifacts (
  id          TEXT PRIMARY KEY,
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  key         TEXT NOT NULL,
  bytes_path  TEXT NOT NULL,              -- relative to artifact root
  size_bytes  INTEGER NOT NULL,
  mime        TEXT,
  created_at  INTEGER NOT NULL,
  UNIQUE(session_id, key)
);

CREATE TABLE events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,  -- monotonic Last-Event-ID
  session_id  TEXT,                                -- nullable for global events
  kind        TEXT NOT NULL,
  payload     TEXT NOT NULL,
  created_at  INTEGER NOT NULL
);
CREATE INDEX idx_events_session_id ON events(session_id, id);

CREATE TABLE permission_decisions (
  id          TEXT PRIMARY KEY,
  session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  ask_id      TEXT NOT NULL,
  permission  TEXT NOT NULL,              -- e.g. 'bash', 'edit:**/*.rs'
  decision    TEXT NOT NULL CHECK(decision IN ('allow','deny','always','never')),
  created_at  INTEGER NOT NULL,
  UNIQUE(session_id, ask_id)
);
```

**Part payload JSON shapes** (centralized in `openlet-core::types::part`):
- `text`: `{"text": "..."}`
- `reasoning`: `{"text":"...", "signature": null|"..."}`
- `tool_call`: `{"call_id":"...","name":"bash","args":{...},"state":"completed","output":{...},"error":null,"started_at":..., "ended_at":...}`
- `step_finish`: `{"reason":"end_turn|tool_use|max_tokens|cancelled","usage":{"prompt":..,"completion":..,"cost_usd_decimal":"0.0024"}}`
- `compaction`: `{"summary":"...","compacted_message_ids":[...],"original_token_count":...}`
- `image`: `{"artifact_ref":"...","mime":"..."}`
- `file`: `{"artifact_ref":"...","filename":"..."}`

**LLM message projection** (`openlet-core::projection`):
```rust
pub fn project_for_llm(msgs: &[Message], parts_by_msg: &HashMap<MessageId, Vec<Part>>) -> Vec<LlmMessage>
```
Rules:
1. `system` messages → preamble (joined into Anthropic-cache-friendly two-segment system prompt later in phase-03).
2. `user` text/image parts → single `user` message; multiple parts concatenated as content array when provider supports it, else string-joined.
3. `assistant` parts: `text` → assistant content; `tool_call` (state=completed) → assistant `tool_calls` entry; `tool_call` output → following `tool` role message keyed by `call_id`.
4. `reasoning` parts dropped UNLESS provider model supports thinking-back (Anthropic) — gated by `provider.capabilities()`.
5. `step_finish` and `compaction` dropped from LLM view but visible in TUI.
6. After compaction: messages whose IDs are in any `compaction.compacted_message_ids` are replaced by the compaction summary. Pruning happens HERE, not in storage.

**Why projection is centralized:** opencode does it inline in the session loop and pays for it (test surface fragmented). claw-code's `api/src/conversation.rs` centralizes it — we mirror that.

## Related Code Files

**Create:**
- `crates/openlet-core/migrations/0001_init.sql`
- `crates/openlet-core/src/types/{part.rs (extend),message.rs (extend),event.rs (extend)}`
- `crates/openlet-core/src/projection.rs`
- `crates/openlet-adapters/src/sqlite/{mod.rs,memory_store.rs,event_repo.rs,permission_repo.rs}`
- `crates/openlet-adapters/src/localfs/{mod.rs,artifact_store.rs,session_log.rs}`
- `crates/openlet-server/src/app_state.rs` — wire concrete adapters

**Modify:**
- `crates/openlet-adapters/Cargo.toml` — enable sqlx features per researcher manifest
- `crates/openlet-server/src/main.rs` — call `sqlx::migrate!` at boot, init artifact dir
- Root `.gitignore` — add `~/.openlet/`-equivalent local dev paths

**Delete:** none.

## Implementation Steps

1. **sqlx setup.** Add `sqlx = { version="0.8", features=["runtime-tokio","tls-rustls","sqlite","migrate","macros","json","chrono","uuid"] }` to `openlet-adapters`. Set `DATABASE_URL=sqlite://./dev.sqlite` for `cargo sqlx prepare`. Commit `.sqlx/` cache after prepare so CI builds offline.
2. **Migration 0001.** Write the schema above. Test with `sqlx migrate run` against a temp file.
3. **Domain types extension.** In `openlet-core::types::part`, define the `Part` enum + per-variant payload structs with `serde`. Centralize JSON encoding in `Part::to_json`/`from_row` so SQLite columns stay schemaless-typed.
4. **`SqliteMemoryStore`.** Implement `MemoryStore`. Use `sqlx::query!` macros (compile-time checked). All write methods take `&self` and acquire from a shared `Pool<Sqlite>`. `append_message` + `append_part` MUST be invocable inside an outer transaction passed via a `Tx` newtype wrapper so phase-03's loop can batch a turn.
5. **Per-session monotonic seq.** Use `INSERT ... RETURNING (SELECT COALESCE(MAX(seq),0)+1 FROM messages WHERE session_id=?)` to keep seq generation in-DB without race conditions.
6. **Event repo.** Append-only writer for the `events` table. Returns the autoincrement `id` (used as Last-Event-ID by SSE in phase-05). Provide `list_since(session_id, after_id)` for resume.
7. **Permission decisions repo.** Read/write `permission_decisions`. Used by ConfigPermissionMgr in phase-04 to honor "always allow" choices across turns.
8. **`LocalFsArtifactStore`.** Implement `ArtifactStore`. Sanitize keys via `sanitize_filename` semantics (no `..`, no leading `/`). `put` writes to `<root>/<session>/<sha256-of-key>` and returns `ArtifactRef { id, key, mime, size_bytes }`. Tracks metadata in `artifacts` table.
9. **JSONL session log.** `SessionLogger` wraps a `tokio::sync::Mutex<BufWriter<File>>`. Append one redacted JSON line per event. Rotate when file exceeds 64MB (rename to `.jsonl.1`, open fresh). Redact via a static `SENSITIVE_KEYS = ["api_key","Authorization","x-api-key","password","secret","token"]` recursive walker.
10. **LLM projection.** Implement `project_for_llm` per Architecture rules. Add table-driven tests covering each Part variant + interleavings. Property test (proptest, optional): "appending a part never changes prior projection prefix".
11. **Wire into AppState.** Replace stub adapters in `openlet-server/src/main.rs` with concrete `SqliteMemoryStore::new(pool)`, `LocalFsArtifactStore::new(root)`. Tie in the migration call before `axum::serve`.
12. **Smoke test.** Add `crates/openlet-server/tests/storage_smoke.rs` that boots the server in-process, calls `MemoryStore` directly, asserts JSONL mirror exists.

## Reference Cross-Check (MANDATORY before coding)

Spawn parallel exploration subagents on:
- **opencode**: `packages/opencode/src/session/message.ts` (Part enum shape, especially `tool` state machine), `packages/opencode/src/session/index.ts` (compaction payload), `packages/opencode/src/storage/storage.ts` (their on-disk layout — JSON-per-message vs SQLite trade-off they made).
- **claw-code**: `rust/crates/runtime/src/session_log.rs` (JSONL rotation + redaction), `rust/crates/api/src/conversation.rs` (projection rules — read closely, especially `tool_calls` <-> `tool` role pairing), `rust/crates/runtime/src/error.rs` (their secret-redaction allowlist — port it).

Confirm or revise: schema column types (esp. INTEGER vs TEXT for timestamps), part JSON shapes, projection rule for reasoning parts when model lacks thinking-back, JSONL rotation threshold.

## Success Criteria

- [ ] `cargo sqlx prepare --workspace` succeeds; `.sqlx/` cache committed
- [ ] Migration runs idempotently — second `cargo run` no-ops
- [ ] `MemoryStore` round-trip test: create session → append 5 messages with mixed parts → `list_messages` returns identical order
- [ ] `LocalFsArtifactStore` round-trip test: put bytes → get returns identical bytes; key with `../etc/passwd` rejected
- [ ] `project_for_llm` table-driven tests pass for each Part variant
- [ ] JSONL log file exists after a session and contains one redacted line per event
- [ ] JSONL rotation works at 64MB threshold (synthetic test)
- [ ] Secret redaction test: planted `"api_key":"sk-XXX"` in event payload → JSONL line has `"api_key":"<redacted>"`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| sqlx offline cache drift in CI | H | M | `cargo sqlx prepare` as pre-push hook; CI fails if cache stale |
| Part JSON schema churn breaking existing rows | M | H | Version field on every part payload; migration script reads + rewrites if version bumps |
| Secret leak via JSONL | M | H | Belt-and-braces: redaction allowlist + integration test that plants secrets and greps the file; deny `Debug` on auth headers structs |
| SQLite contention under concurrent sessions | L | M | `PRAGMA journal_mode=WAL` at pool init; pool size bounded by adapter config |
| Artifact key collisions across sessions | L | L | Path includes `session_id` directory; SQL unique on (session_id, key) |
| Compaction breaks projection determinism | M | M | `project_for_llm` is pure; property test it; compaction adds a Part, never mutates older ones |

## Next Steps

Phase 3 (agent loop core) consumes `MemoryStore`, `project_for_llm`, and the `Part` enum. The `events` table feeds phase-05's SSE channel.
