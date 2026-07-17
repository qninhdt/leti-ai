# Phase 6 — Cloud Filesystem gRPC + pg_trgm (cook)

Date: 2026-07-09
Plan: `plans/260708-1550-emulated-bash-python-executors` — Phase 6
Scope: CROSS-REPO (leti-ai Rust + leti file-service Go). User owns both.

## Goal

Cloud `Filesystem` impl so the emulated bash/python interpreters run on cloud
data (Postgres + S3) with NO container / NO data clone. Local vs cloud differ
ONLY in the injected `Filesystem` impl; interpreters are byte-identical.

## What shipped

### Backend (leti file-service, Go, branch `dev`)

- **Migration `000016_extracted_text_trgm`**: `CREATE EXTENSION pg_trgm` +
  partial GIN index `idx_files_extracted_text_trgm ON files USING GIN
  (extracted_text gin_trgm_ops) WHERE deleted_at IS NULL AND status='ready'`
  (mirrors existing `idx_files_search_gin` predicate). Idempotent.
- **`repo.GrepFiles`** (`internal/repo/files_grep.go`): LITERAL-only prefilter
  — `extracted_text ILIKE ANY($lits)` over `%literal%` fragments, `SET LOCAL
  statement_timeout=3s` scoped to a tx (never leaks to pooled conn), `max_hits`
  ceiling 2000. Returns id+name+folder_id+extracted_text. The caller regex
  NEVER reaches Postgres — ReDoS closed by construction. `escapeLike` neutralizes
  `% _ \`.
- **`service.GrepFiles`** (`internal/service/grep.go`): gates on
  `permission.ActionReadWorkspace` (same as `Search`).
- **`GrepFiles` RPC**: added to TypeSpec source `packages/specs/services/file/
  proto.tsp` (proto is generated — DO NOT EDIT the `.proto`), regenerated via
  `make spec && make proto` (buf 1.69). `remove`/`rename` reuse EXISTING
  `DeleteFile`/`MoveFile`/`PatchFile` RPCs — no new mutation RPCs needed
  (plan over-counted).
- **Tests** (`files_grep_pgintegration_test.go`, 5 cases): literal prefilter,
  case-insensitive superset, empty-literals ready-set, LIKE-metachar escaping,
  ReDoS-input-harmless. Ran green vs real Postgres 16 (docker compose pg,
  container IP DSN). Full repo pgintegration suite + migration harness green.

### leti-ai (Rust)

- **New gRPC foundation** (crate had ZERO): added `tonic`/`prost`/`prost-types`
  + `tonic-build`, vendored proto at `crates/leti-adapters/proto/`, `build.rs`
  codegen (client-only). leti-ai was NOT "a thin gRPC client" as the plan
  said — this is from-scratch plumbing.
- **`cloudfs/literals.rs`** — ReDoS-safe literal extraction. Delegates to
  `regex_syntax` HIR prefix-literal extraction (NOT a hand-rolled scanner).
  Covering invariant: return literals only when EVERY match provably contains
  one; else empty → full-scan. Drops the WHOLE set if any member < 3 chars
  (trigram floor) or is non-UTF-8 (inexact byte-prefix truncated mid-codepoint).
- **`cloudfs/rematch.rs`** — phase-2 in-process linear-time re-match using the
  SAME `RegexBuilder` + truncation as `LocalFilesystem::grep` → dialect parity
  (RE2 both sides; backrefs rejected identically).
- **`cloudfs/mod.rs`** — `CloudFilesystem: Filesystem` over file-service gRPC:
  - `grep` 2-phase (literal prefilter RPC → in-proc re-match), full path
    reconstruction from folder tree for hit parity + `path_glob`.
  - `glob`/`list` via paginated folder-tree walk.
  - `read`/`stat` via `GetFile(include_text)`; `read` errors `Binary` on binary
    files rather than silently returning empty bytes.
  - `remove`→`DeleteFile`, `rename`→`MoveFile`+`PatchFile` (path→id resolve).
  - `write`/`append` STUBBED → `FsError::Unsupported` (per user: read-path full,
    mutations stubbed; presigned-PUT dance + HTTP client out of scope).
  - session-dirty union machinery built (reserved for write/append).
  - bearer JWT in `authorization` metadata; workspace fixed at construction.
- **Feature flag** (`config.rs`): `cloud_fs: Option<CloudFsConfig>`, env-driven
  (`LETI_CLOUD_FS_ENDPOINT/_WORKSPACE_ID/_BEARER`), OFF by default. Partial
  config = hard `ConfigError::Invalid` (fails loud, no silent local fallback).
  `adapter_stack.rs` selects `CloudFilesystem` vs `LocalFilesystem`.
- **`FsError::Unsupported`** variant added → maps to `ToolError::Unimplemented`.

## Verification

- file-service: 5 grep tests + full pgintegration suite + migration harness
  green vs real Postgres 16.
- leti-ai: 788 tests pass (incl 16 new cloudfs unit tests). Workspace build
  clean; cloudfs clippy-clean (pre-existing collapsible-if / is_multiple_of
  warnings from the 1.96 toolchain bump are NOT mine, left as-is).

## Code review

Two adversarial passes (code-reviewer agent).

Pass 1 — REQUEST-CHANGES. Blockers, all fixed:
- **C1**: hand-rolled scanner dropped matches for alternation with a
  literal-less branch (`foo|\d+` → `["foo"]` excludes lines matching only `\d+`).
- **H1**: bounded-quantifier interior leaked as false literal (`abc{123}` → `"123"`).
- **H2**: multi-char escape body leaked (`\p{Greek}` → `"Greek"`).
  → All three fixed by replacing the scanner with HIR extraction.
- **H3**: hardcoded `page_size:1000` single-shot → legitimate files past #1000
  resolved `NotFound`; glob/list truncated silently. → Fixed with paginating
  helpers (loop `next_page_token`, `MAX_PAGES` bound).
- Also addressed: M1 (grep hit path reconstruction), M2 (binary read errors
  loud), M3 (removed rename `mark_dirty` double-count), M4 (ambiguous-name →
  explicit error).

Pass 2 — confirmed C1/H1/H2/H3 RESOLVED; caught ONE new bug:
- `from_utf8_lossy` on an inexact byte-prefix truncated mid-codepoint synthesizes
  a phantom U+FFFD that breaks coverage. → Fixed: `str::from_utf8` + drop whole
  set (full-scan) on non-UTF-8. Added regression test.

## Scope deltas vs plan (documented in phase file)

- Proto is TypeSpec-generated (not hand-editable). Only 1 new RPC (`GrepFiles`);
  remove/rename reuse existing RPCs.
- leti-ai needed a full gRPC stack built from scratch (plan assumed one existed).
- Trait is path-based, backend id-based → path→id resolution via folder-tree walk.
- write/append stubbed (user decision). Cloud `read` serves indexed
  `extracted_text` (truncated/text-only), not raw S3 bytes — documented constraint.

## Deploy-ordering contract (MANDATORY, gates Phase 7 cloud e2e)

1. file-service: migration 000016 + `GrepFiles` handler + proto publish → deploy.
2. leti-ai: rebuild against new proto.
3. Cloud mode `OFF` until 1+2 done; real (non-mock) e2e gate before prod ON.
Against an older backend, `GrepFiles` → gRPC `Unimplemented` → `FsError::Io`
with a deploy-skew message.

## Open questions / follow-ups

- write/append presigned-PUT path (+ HTTP client) deferred — not in this phase.
- No live gRPC transport test (only Postgres was stood up); the tonic wiring is
  compile-verified + unit-tested, transport exercised at Phase 7 e2e.
- M5 (Unicode case-fold: `ILIKE` narrower than Rust `(?i)` simple-fold) noted as
  a rare parity gap; not fixed (full-scan on `(?i)` corpus would be the strict fix).
- Nothing committed (ask-first). 2 repos have staged-worthy changes.
