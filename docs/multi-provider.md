# Multi-provider routing

Phase 5 wires a prefix-based provider router so cloud deployments can serve multiple LLM backends from a single binary without duplicating configuration. This page covers the routing matrix, the BYOK pattern for per-workspace credentials, and the follow-up plan for native Anthropic/Gemini adapters.

## Routing matrix (prefix-strict)

The router (`openlet-adapters::multi_provider::MultiProvider`) selects a backend by inspecting the model name prefix:

| Model prefix          | Backend         |
|-----------------------|-----------------|
| `claude-`             | Anthropic       |
| `anthropic/`          | Anthropic       |
| `gemini-`             | Gemini          |
| `google/`             | Gemini          |
| `gpt-`, `o1-`, `o3-`  | OpenAI-compat   |
| `grok-`               | OpenAI-compat   |
| `kimi-`               | OpenAI-compat   |
| `qwen-`               | OpenAI-compat   |
| `deepseek-`, others   | OpenAI-compat   |

Separators are **strict** — every prefix ends with `-` or `/`. A model named `claude2` does NOT match `claude-`. This prevents collisions on custom OpenRouter model names.

### Collision case

A custom model named `claude-myprovider/foo` syntactically begins with `claude-`, so the default matrix routes it to Anthropic. Integrators with such names override per-prefix:

```rust
use std::collections::HashMap;
use openlet_adapters::multi_provider::{MultiProvider, ProviderKind};

let mut overrides = HashMap::new();
overrides.insert("claude-myprovider/".to_string(), ProviderKind::OpenAiCompat);

let router = MultiProvider::new(anthropic, gemini, openai_compat)
    .with_prefix_overrides(overrides);
```

## Per-provider request shaping

Each backend has request-shape quirks the runtime must absorb. The shaper (`openlet-adapters::openai_compat::prefix_shaping`) detects quirks and rewrites the JSON body before the wire send:

| Family           | Quirk                                              |
|------------------|----------------------------------------------------|
| `gpt-5*`         | Renames `max_tokens` → `max_completion_tokens`     |
| `o1-*`, `o3-*`   | Strips `temperature`, `top_p`, freq/pres penalties |
| `grok-3-mini`    | Same sampling-param strip as o-series              |
| `kimi-*`         | Drops `is_error` from tool result messages         |
| `qwen-*`         | Pre-flight body cap at 6 MiB (DashScope reject)    |
| `gpt-*`, `o*`    | Pre-flight body cap at 100 MiB                     |

The shaper runs unconditionally in `OpenAiCompatProvider::chat_stream`. Custom OpenRouter model names that don't match any quirk prefix pass through unchanged.

## Per-workspace BYOK

Cloud deployments serve multiple workspaces from a single binary. Each workspace can carry its own API keys for any backend (BYOK = bring-your-own-key). The pattern:

1. **`WorkspaceResolver`** trait (in `openlet-server::workspace_resolver`) maps an incoming workspace id to an `Arc<AppState>`.
2. Each `AppState` carries its own `MultiProvider` constructed from the workspace's BYOK keys.
3. The HTTP middleware (`workspace_routing`) reads `x-openlet-workspace` from the request, resolves it via the trait, and inserts the resolved state into request extensions.
4. Handlers extract the per-workspace state via the `WorkspaceRoutingGuard` extractor.

Single-tenant deployments use `StaticWorkspaceResolver`, which always returns the same state — so the local binary keeps booting unchanged.

### Auth ordering contract (F5.1)

The `WorkspaceRoutingGuard` and the routing layer both refuse to proceed unless an `AuthPrincipal` extension is already present in the request. **Mount order MUST be:** auth middleware → workspace_routing middleware → handler. Violating this order produces 401 on every request — loud-fail by design, because skipping auth before workspace lookup is a cross-tenant data exposure.

## Cache markers

`ModelProvider::apply_cache_markers(messages, hint)` lets the runtime hint which conversation boundaries should be cached upstream. Today:

- `OpenAiCompatProvider` — no-op (auto-cache via OpenRouter)
- `AnthropicProvider` STUB — no-op (real impl injects `cache_control: {type: "ephemeral"}` blocks)
- `GeminiProvider` STUB — no-op (Gemini auto-caches; native impl exposes `cachedContent`)

The `Usage` struct carries `cache_creation_input_tokens` (Anthropic alias) alongside `cache_write_tokens` (DashScope/OpenRouter). Cost calc takes the `max()` of the two — providers populate one OR the other, and `max()` bills the cache write exactly once even if a defensive adapter sets both.

## Stub adapters

`AnthropicProvider` and `GeminiProvider` ship today as **stubs** that delegate to `OpenAiCompatProvider`. Both work because OpenRouter accepts the OpenAI-compat shape for `anthropic/*` and `google/*` model names and translates server-side.

Native implementations follow in separate PRs:

- **Anthropic Messages API:** `POST /v1/messages` with top-level `system`, `content` block array, `tool_use` / `tool_result` shapes, ephemeral cache markers.
- **Gemini streamGenerateContent:** `POST /v1beta/models/{model}:streamGenerateContent` with `contents[].parts[]`, `inlineData` / `fileData`, `tools.functionCall` shapes.

Until then, integrators wanting to talk directly to `https://api.anthropic.com/v1/...` cannot use the stub — point at OpenRouter or wait for the native PR.

## Open questions

None — Phase 5 surface complete. TUI cache-hit ratio + notification banner deferred to TUI phase.
