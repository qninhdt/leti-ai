# Codebase Review Fixes — 13-Phase Cook

**Date**: 2026-05-25 15:49
**Severity**: High (security + correctness)
**Component**: openlet-core, openlet-server, openlet-adapters, openlet-protocol
**Status**: Resolved (13/13 phases shipped, 191/191 tests green at every commit boundary)

## What Happened

Six reviewers produced a 1866-line audit (`plans/reports/codebase-review-{A..F,SUMMARY}*.md`).
Synthesized into a 13-phase plan, red-teamed by 4 hostile reviewers (45 raw → 15 deduped
findings folded back in), then cooked end-to-end. Every phase committed with the full
test suite passing.

## The Brutal Truth

Reviewer A's report had drifted from HEAD. If we'd treated its citations as authoritative,
Phase 1 alone would have produced non-compiling code: wrong field names, traits that don't
exist, wrong arities on at least seven distinct symbols. The red team caught all seven by
re-grepping HEAD. **Lesson burned in: reviewer reports are leads, not specs.** Verify
every cited symbol against HEAD before quoting it in a plan.

The cost-cap saga is the other one that stings. `max_cost_per_session_usd` had been
sitting in `Config`, `RuntimeConfig`, and `AgentDefinition` since phase-3 — wired,
serialized, surfaced in env. Decorative the entire time. The substrate (`OnCostTick` +
`cancel_session` + `session_cost`) was sound; the cap never enforced. User decision
2026-05-25: cost cap is cloud-only via quota plugin. Phase 7 ripped the field out and
turned `OPENLET_MAX_COST_USD` into a warn-and-ignore.

## Technical Details — Phase Highlights

| Phase | Win | Closed |
|---|---|---|
| 1 | Atomic `accept_ask` API; `take_deferred` wired through dispatcher; client-pattern injection prevented at writer | BUG-A3, SA-F1, B/I3 |
| 2 | TurnHandle CAS; slot-leak guard via Drop; DELETE cascades cancel; Notify-via-Drop exit signal | C1/C3/C5/C6, FMA-F1 |
| 3 | Event bus `LIMIT 1000` + cursor-too-far rejection (HTTP 409); global SSE replay; `PermissionResolved.session_id` carried through | VULN-F1, B/C1, B/C2, FMA-F3 |
| 4 | Atomic `OpenOptions::create_new`; `MAX_READ_BYTES` floor; grep file/byte caps | VULN-F4, VULN-F5, VULN-F7 |
| 5 | `connect_timeout=10s`, `Retry-After` parse, suppress provider error body, global `DefaultBodyLimit=2 MiB` | HIGH-F1, HIGH-F9, B/I4 |
| 6 | `ChatRequest.headers: BTreeMap<String, String>` (no reqwest coupling); `RESERVED_HEADERS` structural filter | MISS-A1, AD-F11, SA-F3, FMA-F9 |
| 7 | `max_cost_per_session_usd` field deleted | user decision |
| 8 | Atomic INSERT-with-subquery in `append_message`/`append_part` — closes multi-agent fan-out race | B/I2 |
| 9 | Plugin shutdown via `futures::future::join_all` + single 5s timeout (parallel, not sequential) | M1, FMA-F5 |
| 10 | `pending_tool_calls` cap = 64 | ISSUE-A11 |
| 11 | Audit redactor v2: AWS / GCP / GitHub / JWT / Slack / Stripe + whole-name match for sensitive keys | HIGH-F2, SA-F5 |
| 12 | Swagger UI gated by `OPENLET_ENABLE_DOCS` | — |
| 13 | README env table refreshed | — |

## Decisions Worth Remembering

- **Cost cap is cloud-only via quota plugin.** Local runtime has no enforcement.
  The `OnCostTick` callback + `cancel_session` + `session_cost` substrate is intentionally
  preserved so a plugin can implement the cap without core changes.
- **Permission writer = atomic `accept_ask`, not `peek_ask` + `reply_pattern`.** The
  red team (SA-F1) showed the peek-then-reply path opens a TOCTOU window where a client
  pattern can be substituted between peek and write. Atomic write at the resolver closes it.
- **`ChatRequest.headers: BTreeMap<String, String>`**, not `HashMap<HeaderName, SecretString>`.
  Original AD-F11 wanted `HeaderName` for type safety; modified-accept lands at strings to
  keep `openlet-core` free of `reqwest`/`http` types. Adapter validates at the boundary.
- **Dropped `OPENLET_CORS_PERMISSIVE_LOOPBACK_OK`.** Red team correctly flagged it as
  defeating VULN-F6 entirely. No escape hatch.
- **`Notify::notify_waiters` at exit point was rejected.** It has no permit storage, so
  the listener side races: notify fires before the listener subscribes → permanent hang.
  Correct primitive is a `Drop` guard on the spawned task that calls `notify_waiters`
  on drop. If the awaiter has already given up, the drop is a no-op — single-shot and
  resolves immediately. This is the FMA-F1 fix.

## Genuinely Deferred (real scope cuts, not flinches)

- **BUG-A1** reasoning signature — needs `Part` schema migration.
- **HIGH-F5** doom-guard fingerprint rewrite — invasive `turn_loop` refactor.
- **HIGH-F8 / HIGH-F3** turn semaphore + eviction sweep — needs new `eviction.rs` module.
- **VULN-F3** `O_NOFOLLOW` writes — needs `nix::fcntl` integration.
- **M2/M3/M4/I4** plugin registry polish — needs registry surgery.
- Tracing spans + metrics events (Phase 12) — out of scope for this cook.
- All Reviewer-A nits — cosmetic, low risk.

## Surprises Worth Recording

1. **Reviewer-A drift.** Seven citations didn't resolve at HEAD. Field names off,
   traits non-existent, arities wrong. The pattern: reviewer reports age fast, and they
   age silently. Always re-verify with `codegraph_search` or grep before lifting a
   citation into a plan step.
2. **Migration numbering collision.** Phase 6/8 wanted to add `0002`–`0005` but HEAD
   already has `0001` + `0004`. Renumbered: Phase 6 → `0005`, Phase 8 → `0006`–`0008`.
   Migrations didn't actually land in this cook (deferred), but the plan errata was
   corrected so the next pass starts from a valid baseline.
3. **Cost cap was a phantom feature.** No enforcement code, no tests asserting it
   stopped a session, just a serialized number nobody read. The "fix" turned out to be
   "delete the field" — a useful reminder that auditing for unused fields is sometimes
   more valuable than auditing for bugs.

## Lessons Learned

- **Verify reviewer-report citations against HEAD before treating them as authoritative.**
  This is now a sticky rule.
- **Red-teaming a plan is cheap and the ROI is enormous.** 4 hostile reviewers caught 15
  defects that would have been merge-blockers downstream. Budget for it on every plan
  with > 5 phases.
- **Scope cuts are real work.** The deferred list above represents 7 distinct findings
  with concrete blast-radius assessments — not "we got tired." Each carries a one-line
  reason for deferral so the next cook session knows why.
- **`Notify` has sharp edges.** If you reach for it and there's no permit storage, you
  want a different primitive — `oneshot`, a `Drop` guard, or a watch channel. Document
  this so future me doesn't relearn it.

## Next Steps

- Open follow-up phase for migrations `0005`–`0008` (renumbered) — owner: next cook
  session, no urgency.
- Open follow-up phase for `eviction.rs` module (HIGH-F3 + HIGH-F8) — owner: TBD.
- File issues for BUG-A1 (`Part` schema migration), VULN-F3 (`O_NOFOLLOW`), M2/M3/M4/I4
  (plugin registry) so they don't get lost.
- Consider a one-time sweep for "decorative fields" — config keys that serialize but no
  reader consumes. Cost cap won't be the last one.

## Plan Artifacts

- Plan dir: `plans/20260525-1549-codebase-review-fixes/`
- Reports: `plans/reports/codebase-review-{A,B,C,D,E,F,SUMMARY}*.md`
- Final commit boundary: 191/191 tests pass.
