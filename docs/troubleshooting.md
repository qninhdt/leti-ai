# Troubleshooting

A field guide for common failures. For anything not covered here, run
`openlet-server audit --session-id <ID>` and post the (already-redacted)
output along with the error.

## Server won't start

### "loading config: invalid bind address"

`OPENLET_BIND` must be `host:port`. The default is `127.0.0.1:8787`.
A bare port number won't work.

### "binding 127.0.0.1:8787: Address already in use"

Another process owns the port. Find and stop it:

```bash
lsof -i :8787
```

Or pick a different port: `OPENLET_BIND=127.0.0.1:8788 cargo run -p openlet-server`.

### "running sqlite migrations: …"

Usually a corrupt or partially-written `db.sqlite` from an older crash.
Back it up if it has data you care about, then move it aside:

```bash
mv ~/.openlet/db.sqlite ~/.openlet/db.sqlite.broken
```

The next start will recreate it.

## Streaming errors

### `provider_auth` 401

`OPENROUTER_API_KEY` is missing or wrong. The error envelope's `class` is
`provider_auth`. Set the env var and retry — keys are read at startup,
so restart the server after changing it.

### `provider_rate_limit` 429

OpenRouter is throttling. The retry is currently 1s; if this is steady
state, lower `OPENLET_DEFAULT_MODEL` to a less-constrained tier or stop
parallel sessions.

### `context_window` 413

The conversation outgrew the model's window. Compaction should kick in
automatically; if it doesn't, manually trim by starting a fresh session
or using the TUI's `/compact` shortcut.

### `provider_network` 502

Upstream connect / decode failure. Re-run with `RUST_LOG=debug` to see
the actual reqwest error.

### Stream cuts off mid-turn

Look for `class=provider_cancelled` or an `idle timeout` error. The
provider has a 60s no-bytes timeout — if your model genuinely takes
longer between tokens, raise `STREAM_IDLE_TIMEOUT_MS` in
`openai_compat/provider.rs`.

## Tool failures

### `tool_path_outside_workspace`

A tool received a path outside the agent's workspace root. By design.
Use a relative path or extend the agent's workspace via its `AgentSpec`.

### `tool_permission_denied`

The shell or file tool was blocked by `ConfigPermissionMgr`. Either:
- approve the specific call interactively in the TUI, or
- add a rule to the permission config (always-allow / always-ask /
  always-deny) — see `crates/openlet-adapters/src/config_perm/`.

### `tool_read_before_write`

`edit` / `write` requires the file to have been `read` in the same
session first (an invariant that catches accidental clobbers). Read it
once, then re-issue the edit.

### `tool_file_too_large`

`read` rejects files past its byte budget. Use `grep` with a pattern, or
read a slice with `--limit`/`--offset`.

## TUI

### "Cannot connect to server"

The TUI talks to `http://127.0.0.1:8787` by default. If you set
`OPENLET_BIND` to a non-default, the TUI needs the matching base URL
(currently a build-time const — set `OPENLET_API_URL` env var if
the TUI honors it; otherwise rebuild).

### Garbled rendering / no color

Set `TERM=xterm-256color`. Basic 16-color terminals are best-effort;
modal UI still works, gradients won't.

### Windows

Best-effort in MVP. Linux + macOS are tested.

## Audit / forensics

`openlet-server audit --session-id <UUID>` re-redacts and pretty-prints
the JSONL session log. Use `--from` / `--to` (RFC3339) to narrow the
window, and `--format json` for piping into `jq`.

If you suspect the writer leaked a secret past the allowlist, paste the
problematic line into a unit test against `SecretRedactor` — that's the
fast path to either confirm the redactor catches it (file is fine) or
extend the regex (file needs re-redaction on read).

## CI hygiene

If `cargo deny check` starts failing on a brand-new advisory, add an
`ignore = [..]` entry to `deny.toml` with a one-line rationale, then
schedule a dependency bump. Don't silence the entry without filing the
follow-up.

## Still stuck?

Open an issue with:
- `openlet-server --version`
- The `class` from the error envelope
- The last ~20 lines of an audit dump (already redacted)
