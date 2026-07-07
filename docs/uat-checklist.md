# Openlet UAT Checklist

Manual user-acceptance test the operator runs against the **one-command
launch** (`./openlet-ai`), exactly as a customer would. Covers all five
feature-matrix items. Each item lists exact steps, the expected observable
outcome, a checkbox, and an "If it fails" pointer.

Automated coverage backs every item (see `docs/testing-conventions.md`),
but TUI rendering and interactive feel can only be confirmed by a human —
that is what this checklist is for.

## Setup

```bash
cp .env.example .env        # then fill OPENAI_API_KEY
./openlet-ai                # real OpenRouter
# or, no key / no network:
./openlet-ai --mock         # in-process mock LLM
```

Expected: the launcher builds the server, prints a `doctor` preflight, waits
for health, builds the TUI if needed, and drops you into the live terminal
app. On `Ctrl-C` the server is torn down (no orphaned process on the bind
port — verify with `lsof -i :8787` after exit; should be empty).

- [ ] `./openlet-ai` reaches a live TUI in one command (filled `.env`)
- [ ] `./openlet-ai --mock` reaches a live TUI with no key and no network
- [ ] After `Ctrl-C`, the bind port is free (no orphan server)

If it fails: see `docs/troubleshooting.md` → "launcher: port in use" /
"model unreachable / wrong base URL".

---

## Item 1 — Flawless TUI UX/UI

Rendering and input handling can't be auto-asserted; verify by hand.

Steps:
1. Resize the terminal window mid-stream (drag a corner while a long answer
   is printing).
2. Scroll back through a long output (mouse wheel / terminal scrollback).
3. Paste a multi-line block into the prompt.
4. Press `Up`/`Down` to walk prompt history.
5. Type `/` then `Tab` to trigger slash-command completion.

Expected: no visual corruption on resize; scrollback intact; multi-line
paste preserved; history recalls prior prompts; `Tab` completes commands.
Input stays responsive throughout.

- [ ] Resize mid-stream — no corruption
- [ ] Scrollback — prior output intact
- [ ] Multi-line paste preserved
- [ ] `Up`/`Down` history works
- [ ] `Tab` completes slash commands

If it fails: capture the terminal + `$TERM`; TUI rendering issues are
client-side (`tui/src/`), not server.

---

## Item 2 — Native File System Agent (create / read / edit / delete)

The agent's file tools are `write`, `read`, `edit` (deletion is a `bash`
`rm`). Default mode asks before writes — switch to danger mode first so the
agent can act without a prompt per call.

Steps:
1. In the TUI, run `/danger` (or answer "always allow" at the first modal).
2. Prompt: `create a file hello.txt containing "hi"`.
3. Prompt: `read hello.txt back to me`.
4. Prompt: `append a second line "bye" to hello.txt`.
5. Prompt: `delete hello.txt`.
6. Check the workspace on disk between steps:
   `ls "$OPENLET_WORKSPACE"` (default `~/.openlet/workspace`).

Expected: `hello.txt` appears after step 2 with `hi`; step 3 echoes the
content; step 4 adds the line on disk; step 5 removes the file.

- [ ] create → file exists on disk with expected content
- [ ] read → agent reports the content
- [ ] edit/append → new content on disk
- [ ] delete → file gone from disk

If it fails: confirm the session is in danger mode (`/danger`); a write
that hangs is the permission gate parking — answer the modal, or set danger.

---

## Item 3 — LLM Streaming (non-blocking, no glitches)

Steps:
1. Prompt for a long answer: `explain how TCP congestion control works in
   detail`.
2. Watch tokens arrive incrementally (not one big block at the end).
3. While it streams, confirm the UI still accepts input (the status bar
   updates, the spinner animates).
4. Press the cancel binding (or `/cancel`) mid-stream.

Expected: text streams token-by-token, the UI never freezes, and `/cancel`
stops the stream promptly with a clean terminal state.

- [ ] Tokens stream incrementally
- [ ] UI stays responsive during the stream
- [ ] `/cancel` interrupts cleanly

If it fails: see `docs/troubleshooting.md` → "turn hangs forever".

---

## Item 4 — Session & Context persistence across restarts

Steps:
1. Run one turn (any prompt) so the session has history.
2. `/quit` the TUI (server tears down).
3. Relaunch: `./openlet-ai` (same `OPENLET_DATA_DIR`).
4. Run `/sessions` and open the prior session.

Expected: the prior session is listed and its message history is intact —
sqlite under `OPENLET_DATA_DIR` survived the restart.

- [ ] Prior session listed after relaunch
- [ ] Its message history is present

If it fails: confirm `OPENLET_DATA_DIR` is the same across both launches
(default `~/.openlet`); a fresh dir means a fresh database.

---

## Item 5 — Plugin System (dynamic load + ≥1 functional plugin)

Steps:
1. In the TUI, run `/plugins`.
2. Confirm `core-tools` and `core-agents` are listed.
3. Trigger a tool that a plugin provides: prompt `run the bash command:
   echo plugin-ok` (danger mode or answer the modal).

Expected: the plugin list shows `core-tools` + `core-agents` (health OK);
the bash tool actually executes and returns `plugin-ok` — proving a
plugin-contributed tool runs end to end.

- [ ] `/plugins` lists `core-tools` + `core-agents`
- [ ] A plugin-provided tool (bash) executes and returns output

If it fails: `/plugins` empty means the serve path didn't register the
registry — confirm you're on a build that wires `plugin_registry` into the
serving `AppState` (the `/v1/plugin` route reads it).

---

## Sign-off

- [ ] All five items pass against `./openlet-ai` (real OpenRouter)
- [ ] All non-network items pass against `./openlet-ai --mock`
- Operator: ________________   Date: ____________   Build/commit: ____________
