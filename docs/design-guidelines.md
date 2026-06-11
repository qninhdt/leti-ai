# Design Guidelines

_Last updated: 2026-06-11_

The design rules that keep openlet-ai cohesive and cloud-integration-ready.
These are architectural/API design guidelines (not UI design — the only UI is
the terminal client, covered in `tui/`).

## Port/adapter (hexagonal) discipline

- Every external dependency (model, storage, events, permissions, filesystem,
  tool execution) sits behind a **port trait** in `openlet-core/src/adapters/`.
- Core depends on the trait, never a concrete impl. `openlet-adapters` provides
  the local impls; cloud impls live in the openlet repo.
- A port method's signature must be expressible by a remote backend: async,
  workspace/session-scoped where relevant, paginated where unbounded, no
  `std::path` assumption that a remote workspace can't honor.

## Widening a contract

When a cloud need shows the current shape blocks it:

1. Prefer an **additive default method** (e.g. `get_stream` defaulting to
   buffered `get`, `list_*_paged` defaulting to slicing `list_*`,
   `publish_routed` defaulting to `publish`). Existing implementors — including
   the ~50 test doubles — inherit it unchanged.
2. Only the local production impl overrides for real behavior; its existing
   test suite is the regression gate AND the cloud-impl reference.
3. A genuine signature change is atomic (trait + all impls in one commit).
4. Document the behavior a cloud impl must satisfy in the integration guide's
   contract spec — back every claim with a local-impl test.

## Seams, not new mechanisms

- Don't add a second mechanism for something an existing seam covers. Quota is
  the plugin cost-tick seam (not a `Quota` trait); metrics export is the
  `metrics` facade's `Recorder` (not a `MetricsSink` trait); the integrator
  event tap is `EventSink::subscribe` (not a bespoke audit port).
- Identity is a layered seam: `Authenticator` (who is calling) →
  `WorkspaceResolver` (which sandbox, ownership-checked) → `CredentialProvider`
  (what an agent carries outbound). Each has a local default; cloud plugs in.

## Local is plain software

- Running locally must require **no infrastructure**: no Prometheus, no
  database server, no auth server, no compose. Defaults make this true —
  metrics emission is a no-op until a recorder installs, `/metrics` is off
  unless bound, auth is the dev authenticator, storage is SQLite + localfs.
- Opt-in, never opt-out: an operator turns features on via env (`OPENLET_*`),
  they are never required to turn infra off.

## Fail-closed on security boundaries

- Cloud runtime profile with no real authenticator → boot refuses (fail-closed).
- A non-loopback bind with the dev authenticator → boot refuses (a warn is not
  access control).
- Cross-workspace access must be rejected at the resolver; metrics carry no
  per-workspace label on the open scrape (tenant enumeration / cardinality DoS).

## Streaming + concurrency invariants

- The SSE event id is assigned, persisted, and broadcast under one lock so
  arrival order == id order (a `Last-Event-ID` consumer never skips).
- Transient deltas (`part.delta`, `heartbeat`) are never persisted; durable
  events carry a monotonic id seeded from `MAX(id)` (survives restart).
- Turn cancellation cascades: parent cancel → child subagent `child_token`.
- Spans wrap coarse operations (turn, provider call, dispatch), never per-token.

## Plugin extension over core edits

- Tools, agents, and the 14 hook kinds register through `PluginContext`. The
  eight built-in tools ship as `core-tools` to prove the surface suffices.
- Hooks are panic/timeout isolated at every dispatch site; a fault synthesizes
  a `Denied{fault}` and publishes a durable `PluginError` event.

## Documentation accuracy

- No aspirational or false claims (the repo once claimed "Phase 8 CI exists"
  before any CI existed). A doc claim must match the code; the Phase 14
  contract-drift guard keeps the TUI types honest against the server DTOs.
