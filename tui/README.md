# Openlet TUI

SolidJS client (on `@opentui`) for openlet-agent-core. Connects to an
`openlet-server` instance over REST + SSE; streams agent turns into a
full-screen terminal UI with an OpenCode-style prompt, overlay dialogs, and
`@`-file mentions.

> **Runtime:** requires [Bun](https://bun.sh) >= 1.3 — `@opentui/core` uses
> native FFI that Node does not provide.

## Quickstart

```bash
cd tui
bun install
# Server must be running locally on http://127.0.0.1:8787
npm run codegen   # regenerate src/api/schema.d.ts from /v1/doc/openapi.json
npm run build
bun dist/cli.mjs
```

After `npm pack` you can `npm i -g <tarball>` to get the `openlet`
binary on `$PATH`.

## Environment

| Var | Default | Purpose |
|---|---|---|
| `OPENLET_BASE_URL` | `http://127.0.0.1:8787` | Server URL |
| `OPENLET_TOKEN` | — | Bearer token (post-MVP auth) |
| `OPENLET_STATE_DIR` | `~/.openlet` | Prompt history + frecency |

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
  app.tsx            view router + bootstrap + submit
  api/
    client.ts        REST wrapper (replace with openapi-fetch on codegen)
    sse.ts           eventsource@3 + reconnect (header-only Last-Event-ID)
    types.ts         hand-rolled DTOs; replaced by schema.d.ts after codegen
  store/
    index.ts         zustand root + applyEvent
  components/
    status-bar.tsx
    prompt-editor.tsx
    message-list.tsx
    tool-call-card.tsx
    markdown-renderer.tsx
  views/
    chat-view.tsx
    agent-picker.tsx
    session-picker.tsx
    permission-modal.tsx
    plugins-view.tsx
    help-view.tsx
  commands/
    registry.ts      single source of truth for slash commands + /help
  hooks/
    use-throttled-render.ts  33ms flush throttle
    use-prompt-history.ts    JSONL persistence
  theme/
    dark.ts          semantic tokens (truecolor; ink falls back)
  utils/
    markdown-walker.ts       block-safe stream boundary, nested-fence pre-pass
    frecency.ts              opencode-style frequency * 1/(1+days)
    format.ts                USD formatter, short-id
```

## Architecture notes

- Pin Ink 5.2 / React 18.3 — Ink 6 / React 19 had flicker reports during
  the research pass. Upgrade post-MVP.
- SSE wire format mirrors `openlet-server` `/v1/event` exactly:
  `id:` + `event:<dotted.kind>` + `data:<json>`. `Last-Event-ID` is
  header-only (per amendments-after-red-team §C).
- Streaming text deltas append to a per-part `buffer` in the store;
  `useThrottledBuffer` flushes to component state at most once per 33ms
  to keep frame budget under control on 50 tok/s streams.
- Markdown is finalized block-by-block (`utils/markdown-walker.ts`); the
  current pending-tail block renders as plain text until a safe
  boundary (blank line outside fence, or a closing fence of equal-or-
  greater length) is reached.

## See also

- Phase plan: `../plans/20260523-1414-openlet-agent-core-mvp/phase-06-ink-tui.md`
- Cross-check reports:
  - `../plans/20260523-1414-openlet-agent-core-mvp/research/cross-check-phase-06-opencode.md`
  - `../plans/20260523-1414-openlet-agent-core-mvp/research/cross-check-phase-06-clawcode.md`
- Performance gates: `./PERF.md`
