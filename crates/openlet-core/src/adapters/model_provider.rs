use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::ProviderError;
use crate::types::message::Role;

/// Provider-side request body. Phase 3 fills in the message-shaping logic
/// that produces this from a projected conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<LlmMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub tools: Vec<ToolSpec>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Streaming chunk emitted by `chat_stream`. Phase 3 expands variants for
/// reasoning, tool-call accumulation, and finish reasons.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatDelta {
    Text { text: String },
    Reasoning { text: String },
    ToolArgs { call_id: String, args_chunk: String },
    StepFinish { reason: String },
}

/// Per-model pricing (decimal strings to avoid float drift on cost math).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: String,
    pub output_per_mtok: String,
    pub cached_input_per_mtok: Option<String>,
}

/// Wraps an LLM provider — local mock, OpenAI-compat, OpenRouter.
///
/// Implementations MUST be cancellation-aware: dropping `chat_stream` or
/// triggering `cancel` mid-stream MUST tear down upstream connections.
#[async_trait]
pub trait ModelProvider: Send + Sync + 'static {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<Box<dyn Stream<Item = Result<ChatDelta, ProviderError>> + Send + Unpin>, ProviderError>;

    fn pricing(&self, model: &str) -> Option<ModelPricing>;
}
