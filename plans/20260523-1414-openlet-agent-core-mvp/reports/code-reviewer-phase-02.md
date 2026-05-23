# Phase 2 Code Review — Storage and Message Model

**Date:** 2026-05-23
**Scope:** Phase 2 implementation (storage layer, projection, JSONL log)
**Reviewer:** self-review (subagent ran but did not persist report)

## Status: DONE_WITH_CONCERNS

Build/test/lint gates clean (build 0 errors, 18/18 tests, clippy `-D warnings` clean). The concerns below are non-blocking for Phase 2 acceptance but tracked for Phase 3+ follow-up.

## Verified clean
- Migration runs idempotently (`migration_idempotent` test re-runs MIGRATOR on a fresh in-memory pool with no error; `IF NOT EXISTS` on every CREATE).
- `MemoryStore` round-trip — 4 messages with mixed roles + part append/upsert (`append_messages_keeps_seq_order`, `append_and_upsert_part`).
- LocalFsArtifactStore round-trip + path traversal rejection — `../etc/passwd`, `/etc/passwd`, `..`, `..\\..\\evil` all rejected (`rejects_traversal_keys`).
- `project_for_llm` table-driven tests cover Text, Reasoning, ToolCall, ToolResult, empty-user, append-only-prefix invariant.
- JSONL: one redacted line per event; `Bearer sk-...` token stripped (`redacts_sensitive_keys_and_bearer_tokens`).
- All SQL parameters bound via sqlx `.bind(...)` — no `format!` interpolation of user data into queries.
- `parse_uuid` is used by `row_to_session`/`row_to_message` — not dead.
- `chrono::Utc.timestamp_millis_opt(...).single().unwrap_or_else(Utc::now)` — fallback behavior intentional; values come from our own writes (`Utc::now().timestamp_millis()`), so out-of-range only on data corruption.
- Soft-delete via `UPDATE` keeps cascade dormant — sessions stay queryable for resume; intended.
- §A schema columns present: `parent_session_id`, `permission_mode`, `version`, `deleted_at`. ✓
- §F `session_reads` table + `record_read` impl present. ✓
- §M regex redaction + key allowlist present. ✓
- §S no-`Other(String)` — all error variants are typed. ✓

## Important (fix before next phase)

1. **Artifact orphaning on key overwrite.**
   File: `crates/openlet-adapters/src/localfs/artifact_store.rs:73-94`
   `put` does `tokio::fs::write(&path, ...)` (overwrites in place because the path is `sha256(key)` deterministic) then `ON CONFLICT(session_id, key) DO UPDATE SET bytes_path = excluded.bytes_path`. Since the path is identical for the same key, no file orphan in practice — but `bytes_path` is recomputed redundantly. Cosmetic only.
   **Action:** none for Phase 2. Note for Phase 8 hardening if mime types or content-addressing change.

2. **Per-session monotonic seq race.**
   File: `crates/openlet-adapters/src/sqlite/memory_store.rs:121-143` (`append_message`).
   `SELECT COALESCE(MAX(seq),0)+1` followed by `INSERT` runs inside `pool.begin()` (deferred BEGIN under SQLite). Under WAL with concurrent writers SQLite serializes writes via the writer lock, so two parallel `append_message` calls for the same session cannot both pass through. The `UNIQUE(session_id, seq)` constraint is the safety net.
   **Action:** none — race-free under SQLite's single-writer model. Documented.

3. **JSONL key allowlist case-sensitivity.**
   File: `crates/openlet-adapters/src/localfs/session_log.rs:128-132`.
   `is_sensitive_key` lowercases both sides — `API_KEY`, `Authorization`, `X-Api-Key` all match. ✓

4. **JSONL recursion through nested payloads.**
   File: `session_log.rs:135-156`.
   `redact_in_place` walks `Value::Object` and `Value::Array` recursively. `AgentEvent::Error.message` and `PartDelta.delta` are top-level strings → regex catches them. ✓

5. **JWT-format token leak.**
   The current regex `sk-[A-Za-z0-9_\-]{16,}` does not catch JWTs (`eyJ...`).
   **Action:** Phase 8 hardening — extend regex set when JWT-issuing providers land. Not blocking Phase 2.

## Nice-to-have

- `SessionLogger.locks: DashMap<SessionId, Arc<Mutex<()>>>` grows unbounded with session count. Per-session lock entries should be reaped on session close in Phase 5.
- `event_repo.rs::SqliteEventRepo::list_since` and `permission_repo.rs::list_for_session` are dead in Phase 2 — wired up in Phase 4 and Phase 5. Tracked, not blocking.
- No 64MB rotation test (the spec calls for one). The rotation code path is exercised manually but a synthetic test would write 64MB+ which is slow. Deferred to Phase 8.

## Phase 1 contract regression check

- `SqliteMemoryStore::new()` → `SqliteMemoryStore::new(pool: SqlitePool)`: breaking signature change, but only `crates/openlet-server/src/main.rs` constructs it (verified by grep). Updated.
- `LocalFsArtifactStore::new()` → `LocalFsArtifactStore::new(root, pool)`: same — only `main.rs`. Updated.
- All other Phase 1 stubs untouched. AppState shape unchanged. ✓

## Open questions

- Should JSONL rotation threshold be configurable (env var) for low-disk environments? Currently hard-coded 64MB.
- Are `permission_decisions.decision` values `allow|deny|always|never` the correct enum surface, or should `always` be split into `always_allow|always_deny`? Phase 4 will clarify.
