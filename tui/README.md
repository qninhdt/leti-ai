# Leti TUI

SolidJS client (on `@opentui`) for leti-agent-core. Connects to an
`leti-server` instance over REST + SSE; streams agent turns into a
full-screen terminal UI with an OpenCode-style prompt, overlay dialogs, and
`@`-file mentions.

> **Runtime:** requires [Bun](https://bun.sh) >= 1.3 — `@opentui/core` uses
> native FFI that Node does not provide.

## Quickstart

```bash
cd tui
bun install
# Server must be running locally on http://127.0.0.1:8787
npm run codegen   # regenerate src/api/schema.d.ts from /doc/openapi.json
npm run build
bun dist/cli.mjs
```

After `npm pack` you can `npm i -g <tarball>` to get the `leti`
binary on `$PATH`.

## Environment

| Var | Default | Purpose |
|---|---|---|
| `LETI_BASE_URL` | `http://127.0.0.1:8787` | Server URL |
| `LETI_TOKEN` | — | Bearer token (post-MVP auth) |
| `LETI_STATE_DIR` | `~/.leti` | Prompt history + frecency |

## Slash commands

`/help`, `/agents`, `/sessions`, `/new`, `/cancel`, `/danger`,
`/plugins`, `/quit` (`/exit`, `/q`).

## Keyboard

- `Enter` — submit
- `Shift+Enter` — newline (Option+Enter on macOS terminals that don't
  pass Shift+Enter through; Ctrl+J also works)
- `Up/Down` — prompt history (cap 200)
- `Tab` — slash-command completion
- `1`/`a` — allow once · `2`/`A` — always · `3`/`r`/`Esc` — deny
  (in permission modal)

## Layout

```
src/
  cli.tsx            entry, mounts <App/>
  app.tsx            route switch + bootstrap + submit
  api/
    client.ts        hand-rolled REST wrapper
    sse.ts           eventsource@3 + reconnect (header-only Last-Event-ID)
    types.ts         hand-rolled DTOs (contract source; see schema.d.ts note)
    schema.d.ts      generated OpenAPI snapshot — reference for contract-drift CI, NOT imported
  store/
    index.ts         zustand root (wires applyEvent + reducers)
    apply-event.ts   pure SSE reducer: (state, ev) => Partial<State>
    reducers.ts      immutable message/part update helpers
    message-hydration.ts  REST-fetched message → store shape
  components/
    prompt-editor.tsx           prompt input layout + wiring
    create-prompt-key-handler.ts  mention/slash/history/interrupt key routing
    use-prompt-derived.ts       agent/model/usage/cost/context memos
    use-interrupt-arm.ts        double-interrupt-to-cancel arm
    message-list.tsx, tool-call-card.tsx, ...
  dialogs/           overlay dialogs (agent picker, permission modal, ...)
  routes/            top-level views switched by app.tsx
  render/            OpenTUI render helpers
  services/          side-effecting client services
  commands/
    registry.ts      single source of truth for slash commands + /help
    builtins/        individual slash-command implementations
  hooks/             shared Solid hooks
  theme/             semantic color tokens
  utils/             frecency, formatters, misc pure helpers
```

## Testing

```bash
npm run typecheck      # tsc --noEmit
npm test               # vitest run — unit + store/parser/hydration suites
```

The suite is self-contained Vitest — no Rust toolchain, no server, no
network. It exercises the pure store reducers (`store/apply-event.ts`),
message hydration, tool/label/output formatting, mention parsing, the
command registry, and the event pump.

## Architecture notes

- Runtime is [Bun](https://bun.sh) — `@opentui/core` uses native FFI Node
  does not provide. The UI is SolidJS on `@opentui/solid`.
- SSE wire format mirrors `leti-server` `/v1/event` exactly:
  `id:` + `event:<dotted.kind>` + `data:<json>`. `Last-Event-ID` is
  header-only.
- Streaming text deltas append to a per-part `buffer` in the store; the
  render layer coalesces buffer growth into frames so a fast stream does
  not thrash the terminal.
- `store/apply-event.ts` is the single mutation point for SSE frames — a
  pure `(state, ev) => Partial<State>` grouped by domain (session, parts,
  overlays/asks, plugins/plan/errors). Transient frames that should not
  mutate durable state are ignored there.

## See also

- Phase plan: `../plans/20260523-1414-leti-agent-core-mvp/phase-06-ink-tui.md`
- Cross-check reports:
  - `../plans/20260523-1414-leti-agent-core-mvp/research/cross-check-phase-06-opencode.md`
  - `../plans/20260523-1414-leti-agent-core-mvp/research/cross-check-phase-06-clawcode.md`
- Performance gates: `./PERF.md`
