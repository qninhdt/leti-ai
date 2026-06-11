use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::Stream;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::ProviderError;
use crate::projection::LlmMessage;
use crate::types::event::Usage;

/// Provider-side request body. Built by the runtime from a projected
/// conversation (`projection::LlmMessage`) plus tool definitions and per-turn
/// sampling params. Wire-agnostic; the OpenAI-compat adapter converts to the
/// concrete OpenAI JSON shape inside its own `wire` module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<LlmMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// Plugin-injected HTTP headers, populated by the `OnChatHeaders`
    /// hook chain. `String` keys (not `HeaderName`) so `openlet-core`
    /// stays decoupled from `reqwest`. `BTreeMap` for stable iteration.
    /// Reserved header names (`authorization`, `x-api-key`, etc.) are
    /// filtered structurally by adapters before merge — see
    /// RESERVED_HEADERS in openai_compat::provider.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

const fn default_stream() -> bool {
    true
}

/// One tool advertised to the model. `parameters` is a JSON Schema (built
/// from `schemars::schema_for!` per registered tool input struct).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Closed-set finish reason. Loop termination policy:
/// - `EndTurn | MaxTokens | Error | Cancelled | Halted` → terminate the loop
/// - `ToolUse` → run tools then continue
/// - `Length | ContentFilter` → terminate (treated as error-ish; bubble up)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Length,
    ContentFilter,
    Error,
    Cancelled,
    /// A `before_turn` / `on_cost_tick` plugin returned `HookResult::Stop`.
    /// Loop terminates without emitting a regular `EndTurn`.
    Halted,
    /// The turn loop hit `LoopContext::max_steps` without the model emitting
    /// `EndTurn`. Distinct from `MaxTokens` (model-side cap) so cost/audit
    /// telemetry can tell them apart.
    MaxSteps,
}

/// Streaming chunk emitted by `chat_stream`.
///
/// Granularity collapses a provider `StreamEvent` set
/// into a flat enum. Tool-call args arrive in chunks indexed by position;
/// the processor accumulates them and parses on `Finish`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatDelta {
    /// Assistant role announcement (some providers send this once at start).
    Role,
    /// Streamed assistant text fragment.
    Content { text: String },
    /// Reasoning preamble (OpenAI o1/o3 `reasoning_content` or Anthropic
    /// `thinking_delta`). `signature` is Anthropic's signed-thinking token.
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// First chunk for a tool call at `index`. Carries `call_id` + `name`.
    ToolCallStart {
        call_id: String,
        name: String,
        index: usize,
    },
    /// Subsequent argument fragment for the tool call at `index`. Concatenate
    /// chunks; do NOT parse until `Finish` (chunks may not be valid JSON
    /// mid-stream).
    ToolCallArgsDelta { index: usize, args_chunk: String },
    /// Terminal frame — `usage` is best-effort (some providers send it on a
    /// preceding `MessageDelta` instead).
    Finish {
        reason: FinishReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
}

/// Per-model pricing. Stored as `Decimal` (USD per million tokens) to avoid
/// f64 drift in cost math.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: Decimal,
    pub output_per_mtok: Decimal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_per_mtok: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_mtok: Option<Decimal>,
}

/// Type alias for the streaming chunk channel — kept as a boxed trait object
/// (revisit when GATs stabilize cleanly across runtime).
pub type ChatStream =
    Box<dyn Stream<Item = Result<ChatDelta, ProviderError>> + Send + Unpin + 'static>;

/// Static, model-aware capability flags. Returned by
/// [`ModelProvider::capabilities`] so the runtime can ask one provider
/// "do you support vision for this model?" instead of hard-coding
/// per-model branches in the projector or request builder.
///
/// All flags default `false` so a provider that doesn't advertise a
/// capability is safe by construction. `max_request_body_bytes = 0`
/// means "no provider-side cap; defer to the global server limit".
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Provider accepts inline image inputs (multimodal vision).
    pub supports_vision: bool,
    /// Provider accepts inline document inputs (PDFs, etc.).
    pub supports_document_input: bool,
    /// Per-image upper bound enforced by the upstream API (0 = unset).
    pub max_image_bytes: usize,
    /// `gpt-5*` family routes the response-token cap through
    /// `max_completion_tokens` rather than `max_tokens`. Adapters
    /// switch the JSON field name based on this flag.
    pub max_completion_tokens_param: bool,
    /// `o1*` / `o3*` reasoning models and `grok-3-mini` reject
    /// `temperature` / `top_p` / `frequency_penalty` /
    /// `presence_penalty`. Adapters drop those fields when this is
    /// true.
    pub strip_sampling_params: bool,
    /// Moonshot Kimi rejects requests carrying an `is_error` field on
    /// tool result messages — adapters omit the field when this is
    /// true. (Other providers tolerate it.)
    pub reject_is_error_field: bool,
    /// Pre-flight body-size cap. DashScope rejects > 6 MiB; OpenAI
    /// stops at 100 MiB. `0` means "no cap; rely on global limit".
    pub max_request_body_bytes: usize,
}

/// Hint for [`ModelProvider::apply_cache_markers`]. The runtime decides
/// which boundaries it'd like cached; the provider chooses how (or
/// whether) to mark them. Anthropic injects `cache_control` blocks;
/// OpenAI-compat / Gemini auto-cache so the call is a no-op there.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheHint {
    pub system_prompt: bool,
    pub tool_definitions: bool,
    pub last_user_turn: bool,
}

/// One model entry returned by [`ModelProvider::list_models`]. Mirrors the
/// common subset of the OpenAI / OpenRouter `GET /models` response — `id`
/// is the only guaranteed field; the rest are best-effort enrichment the
/// catalog may omit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Canonical model id used in `ChatRequest::model` (e.g.
    /// `anthropic/claude-sonnet-4-6`).
    pub id: String,
    /// Human-readable label when the catalog provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Maximum context window in tokens, when advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

/// Wraps an LLM provider — local mock, OpenAI-compat, OpenRouter.
///
/// Implementations MUST be cancellation-aware: dropping `chat_stream` or
/// triggering `cancel` mid-stream MUST tear down upstream connections.
///
/// Cloud-readiness (adapter-contract audit): this trait is already
/// impl-agnostic — `ChatRequest`/`ChatStream` are wire-neutral, streaming
/// is a `Stream` trait object, and `capabilities`/`list_models`/`pricing`
/// carry no local assumptions. A remote provider (OpenRouter, a cloud
/// gateway) is expressible with no signature change; verified by the
/// `OpenRouterProvider` + `OpenAiProvider` impls.
#[async_trait]
pub trait ModelProvider: Send + Sync + 'static {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError>;

    fn pricing(&self, model: &str) -> Option<ModelPricing>;

    /// Fetch the provider's catalog of available models. Default impl
    /// returns an empty list so providers that don't expose a catalog
    /// (mock, self-hosted single-model gateways) don't have to opt in;
    /// the `GET /v1/models` route then returns `[]` rather than erroring.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(Vec::new())
    }

    /// Per-model capability descriptor consulted by the runtime when
    /// projecting a turn. Providers that don't implement vision /
    /// document input return the default (all `false`); the runtime
    /// then rewrites unsupported parts to text fallbacks before
    /// dispatch. Default impl exists so existing providers don't have
    /// to opt in.
    fn capabilities(&self, _model: &str) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    /// Inject prompt-cache markers into `_messages` before the
    /// request is built. Default no-op suits providers that auto-cache
    /// (OpenAI-compat, Gemini). Anthropic stub overrides this once the
    /// native Messages API lands.
    fn apply_cache_markers(&self, _messages: &mut Vec<LlmMessage>, _hint: CacheHint) {}
}
