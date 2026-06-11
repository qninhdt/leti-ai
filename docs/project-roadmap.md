# Project Roadmap

_Last updated: 2026-06-11_

Tracks the production-rehabilitation program (plan
`plans/20260610-1345-openlet-ai-production-rehabilitation/`) and what remains.

## Status legend

✅ complete · 🟡 partial · ⛔ blocked · ⬜ pending

## Phases

| # | Phase | Status | Notes |
|---|---|---|---|
| 1 | Hygiene & comment cleanup | ✅ | |
| 2 | Structure standardization | ✅ | runtime split into focused modules |
| 3 | Core correctness fixes | ✅ | compaction token-lag, subagent finalize race, chat-header wiring |
| 4 | Complete deferred core | ✅ | provider retry/backoff, per-session model, tool-arg streaming |
| 5 | OpenRouter provider | ✅ | two-adapter split (openai + openrouter), enrichment + cost parse |
| 6 | Integration — auth & identity seam | ✅ | Authenticator + CredentialProvider + canonical AuthPrincipal, mounted layers, runtime profile |
| 7 | Integration — adapter contract hardening | ✅ | pagination, streaming/presign, routing/delivery; cloud-readiness audit + contract spec |
| 8 | TUI rehab — agent surfaces | ✅ | Solid/@opentui migration merged; server `GET …/messages` + agent surfaces wired |
| 9 | TUI rehab — polish | ✅ | delivered by the merged Solid migration (OpenCode-style overlays, dialogs, prompt editor) |
| 10 | Telemetry & observability | ✅ | correlated spans + dormant-by-default Prometheus metrics |
| 11 | Testing strategy redesign | ✅ | Rust gaps closed; TUI suite green post-merge (33 vitest passing) |
| 12 | Real-LLM acceptance | ✅ | M18 scrubber + 10 live tests (9 suites) PASS against real OpenRouter — incl. 6 multi-turn/multi-actor scenarios (cross-file refactor, debug-fix-verify, scaffold-discovery, compaction-continuity, ask_user human-in-the-loop, subagent orchestration) |
| 13 | Infrastructure | ✅ | Dockerfile + Compose + env separation (compose config validated) |
| 14 | CI-CD pipeline | ✅ | PR checks + gated nightly + image build; deny.toml tightened; false CI claims removed |
| 15 | Docs & final report | ✅ (this) | 6 mandated docs + final report |

## Blocked / deferred work

- **TUI (Phases 8/9 client side, Phase 11 TUI tests):** the `tui/` tree is
  mid-migration from Ink/React to Solid + @opentui. Defect fixes (permission
  modal port, tool-call render, lagged resume, ask-user modal, user echo,
  session-resume wiring) wait until the migration settles. The server-side
  prerequisite (`GET /v1/session/:id/messages`) is done.
- **Real-LLM acceptance (Phase 12):** the 7 capability tests are written-shaped
  but `#[ignore]`'d + double-gated; running them needs an OpenRouter key and
  costs money. The keyless prerequisite (evidence scrubber) is done.

## Open questions (owner / openlet team)

1. SA credential scope & issuance: per-workspace token vs one SA + workspace claim.
2. Cost-ledger ownership: self-contained Postgres vs a future openlet quota service.
3. Caller set: does it include leti→agent calls? (affects `PrincipalType`).
4. Presigned-URL timing for agent file tools.

## Next milestones

- Land the TUI Ink→Solid migration, then complete Phase 8/9 + TUI tests.
- Run the Phase 12 acceptance suite against a live key; record per-run cost.
- Exercise the CI workflows on a real PR + one manual nightly.
- Resolve the four open questions with the owner / openlet team.
