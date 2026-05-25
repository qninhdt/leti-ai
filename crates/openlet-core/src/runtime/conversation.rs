//! Conversation runtime — drives one provider call end-to-end.
//!
//! Slice-2 scope: a single LLM stream → `Processor` → storage + bus.
//! Tool dispatch and the multi-step outer loop (claw `run_turn` /
//! opencode `runLoop`) land in Phase 4. Cost is tracked per-session
//! across turn calls, so the future loop can enforce `max_cost_per_session`.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use dashmap::DashMap;
use futures::StreamExt;
use rust_decimal::Decimal;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::adapters::event_sink::{EventSink, Persistence};
use crate::adapters::memory_store::MemoryStore;
use crate::adapters::model_provider::{ChatRequest, FinishReason, ModelProvider, ToolSpec};
use crate::dispatch::{DispatchOutcome, HookChains, dispatch, publish_fault_if_any};
use crate::error::{CoreError, ProviderError};
use crate::hooks::io::{OnChatHeadersCtx, OnChatMessagesCtx, OnChatParamsCtx, OnCostTickCtx};
use crate::projection::LlmMessage;
use crate::runtime::cost::{compute_cost, format_usd};
use crate::runtime::processor::{Processor, ProcessorState};
use crate::runtime::prompt::compose_system_prompt;
use crate::runtime::turn_stream::StreamingPartTracker;
use crate::types::event::{AgentEvent, Usage};
use crate::types::message::{Message, MessageId, Role};
use crate::types::session::SessionId;

const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Runtime tunables. Plumbed from `openlet_core::config::Config` at boot.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub idle_timeout: Duration,
    pub default_model: String,
}

impl RuntimeConfig {
    #[must_use]
    pub fn new(default_model: String) -> Self {
        Self {
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            default_model,
        }
    }
}

/// Caller-supplied turn description. The runtime owns request building
/// because (a) `tools` materialization needs the registry, (b) `messages`
/// is the projected conversation, not raw `Part`s.
#[derive(Debug, Clone)]
pub struct TurnInput {
    pub session_id: SessionId,
    pub messages: Vec<LlmMessage>,
    pub system_prompt: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub assistant_message_id: MessageId,
    pub finish_reason: FinishReason,
    pub usage: Option<Usage>,
    pub cost_usd: Option<Decimal>,
}

pub struct ConversationRuntime {
    provider: Arc<dyn ModelProvider>,
    memory: Arc<dyn MemoryStore>,
    events: Arc<dyn EventSink>,
    config: RuntimeConfig,
    session_costs: Arc<DashMap<SessionId, Decimal>>,
    hook_chains: Arc<HookChains>,
}

impl ConversationRuntime {
    #[must_use]
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        memory: Arc<dyn MemoryStore>,
        events: Arc<dyn EventSink>,
        config: RuntimeConfig,
    ) -> Self {
        Self::with_hook_chains(
            provider,
            memory,
            events,
            config,
            Arc::new(HookChains::new()),
        )
    }

    /// Same as [`Self::new`] but accepts a pre-built [`HookChains`] so
    /// `on_cost_tick` (and slice-3c hooks) fire on every turn. Existing
    /// callers without plugin support keep using [`Self::new`].
    #[must_use]
    pub fn with_hook_chains(
        provider: Arc<dyn ModelProvider>,
        memory: Arc<dyn MemoryStore>,
        events: Arc<dyn EventSink>,
        config: RuntimeConfig,
        hook_chains: Arc<HookChains>,
    ) -> Self {
        Self {
            provider,
            memory,
            events,
            config,
            session_costs: Arc::new(DashMap::new()),
            hook_chains,
        }
    }

    /// Cumulative cost recorded across turns of `session_id`. Returns zero
    /// for unknown sessions.
    pub fn session_cost(&self, session_id: SessionId) -> Decimal {
        self.session_costs
            .get(&session_id)
            .map(|v| *v)
            .unwrap_or_default()
    }

    /// Drives one assistant turn. Caller owns `cancel` — pass a child token
    /// of the session token so an external abort cascades into the
    /// provider stream and any spawned work.
    pub async fn run_turn(
        &self,
        input: TurnInput,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, CoreError> {
        let session_id = input.session_id;
        let model = input
            .model
            .clone()
            .unwrap_or_else(|| self.config.default_model.clone());

        let message_id = self.create_assistant_message(session_id).await?;

        // OnChatParams — plugins mutate model / max_tokens / temperature.
        // O(1) skip when no plugin registered the chain.
        let params = if self.hook_chains.on_chat_params.is_empty() {
            OnChatParamsCtx {
                model,
                max_tokens: input.max_tokens,
                temperature: input.temperature,
            }
        } else {
            let params_ctx = OnChatParamsCtx {
                model: model.clone(),
                max_tokens: input.max_tokens,
                temperature: input.temperature,
            };
            match dispatch(&self.hook_chains.on_chat_params, params_ctx).await {
                DispatchOutcome::Completed(c) | DispatchOutcome::Stopped(c) => c,
                DispatchOutcome::Denied {
                    reason,
                    feedback,
                    plugin_fault,
                } => {
                    if let Some(fault) = plugin_fault.as_ref() {
                        let _ = self
                            .events
                            .publish(
                                crate::dispatch::plugin_error_event(Some(session_id), fault),
                                Persistence::Durable,
                            )
                            .await;
                    }
                    tracing::warn!(reason = %reason, feedback = ?feedback, "on_chat_params denied; halting turn");
                    return Err(CoreError::Provider(ProviderError::Cancelled));
                }
            }
        };

        // OnChatMessages — plugins rewrite the message list (compaction,
        // ablation, prompt-prefix injection). O(1) skip when empty.
        // Compose the per-provider overlay onto the caller's system_prompt
        // here, after OnChatParams has resolved the final model. Plugins
        // observing OnChatMessages see (and can rewrite) the composed
        // prompt.
        let composed_system_prompt = Some(compose_system_prompt(
            input.system_prompt.as_deref(),
            &params.model,
        ));
        let messages = if self.hook_chains.on_chat_messages.is_empty() {
            OnChatMessagesCtx {
                model: params.model.clone(),
                system_prompt: composed_system_prompt,
                messages: input.messages,
            }
        } else {
            let messages_ctx = OnChatMessagesCtx {
                model: params.model.clone(),
                system_prompt: composed_system_prompt,
                messages: input.messages,
            };
            match dispatch(&self.hook_chains.on_chat_messages, messages_ctx).await {
                DispatchOutcome::Completed(c) | DispatchOutcome::Stopped(c) => c,
                DispatchOutcome::Denied {
                    reason,
                    feedback,
                    plugin_fault,
                } => {
                    if let Some(fault) = plugin_fault.as_ref() {
                        let _ = self
                            .events
                            .publish(
                                crate::dispatch::plugin_error_event(Some(session_id), fault),
                                Persistence::Durable,
                            )
                            .await;
                    }
                    tracing::warn!(reason = %reason, feedback = ?feedback, "on_chat_messages denied; halting turn");
                    return Err(CoreError::Provider(ProviderError::Cancelled));
                }
            }
        };

        // OnChatHeaders — auth/tracing headers per provider call. Phase
        // 4 widens `ModelProvider::chat_stream` to consume the headers;
        // for now plugins can register and observe (audit logs, metrics)
        // but any `Replace` mutation is silently dropped until phase 4.
        // O(1) skip when empty.
        if !self.hook_chains.on_chat_headers.is_empty() {
            let headers_ctx = OnChatHeadersCtx {
                model: params.model.clone(),
                headers: Vec::new(),
            };
            let headers_outcome = dispatch(&self.hook_chains.on_chat_headers, headers_ctx).await;
            publish_fault_if_any(&self.events, Some(session_id), &headers_outcome).await;
        }

        let req = ChatRequest {
            model: params.model.clone(),
            messages: messages.messages,
            system: messages.system_prompt,
            max_tokens: params.max_tokens,
            temperature: params.temperature,
            tools: input.tools,
            stream: true,
            headers: Default::default(),
        };

        let outcome = self
            .drive_stream(session_id, message_id, params.model, req, cancel)
            .await;

        match outcome {
            Ok(o) => Ok(o),
            Err(e) => {
                self.publish_error(session_id, &e).await;
                Err(e)
            }
        }
    }

    async fn drive_stream(
        &self,
        session_id: SessionId,
        message_id: MessageId,
        model: String,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, CoreError> {
        let mut stream = self.provider.chat_stream(req, cancel.clone()).await?;
        let mut state = ProcessorState::default();
        let mut tracker = StreamingPartTracker::default();

        loop {
            let next = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    return Err(CoreError::Provider(ProviderError::Cancelled));
                }
                slot = timeout(self.config.idle_timeout, stream.next()) => slot,
            };

            let item = match next {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(_) => {
                    return Err(CoreError::Provider(ProviderError::Network(
                        "stream idle timeout".into(),
                    )));
                }
            };

            let delta = item?;
            let step = Processor::step(state, delta)?;
            state = step.next;

            for evt in step.events {
                tracker
                    .handle_event(&self.memory, &self.events, session_id, message_id, evt)
                    .await?;
            }

            if !step.parts.is_empty() {
                let cost_str = self.turn_cost(&model, state.usage.as_ref()).map(format_usd);
                for part in step.parts {
                    tracker
                        .handle_part(
                            &self.memory,
                            &self.events,
                            session_id,
                            message_id,
                            part,
                            cost_str.clone(),
                        )
                        .await?;
                }
            }
        }

        let finish = state.finish.unwrap_or(FinishReason::EndTurn);
        let usage = state.usage.clone();
        let cost = self.turn_cost(&model, usage.as_ref());
        if let Some(c) = cost {
            self.add_session_cost(session_id, c);
        }
        let total_cost = self.session_cost(session_id);

        // OnCostTick — Stop here forces FinishReason::Halted so the
        // turn loop terminates without continuing into another step.
        let cost_ctx = OnCostTickCtx {
            session_id: Some(session_id),
            model: model.clone(),
            delta_usd: cost,
            total_usd: total_cost,
            usage: usage.clone(),
        };
        let cost_outcome = dispatch(&self.hook_chains.on_cost_tick, cost_ctx).await;
        publish_fault_if_any(&self.events, Some(session_id), &cost_outcome).await;
        let final_finish = match cost_outcome {
            DispatchOutcome::Completed(_) => finish,
            DispatchOutcome::Stopped(_) | DispatchOutcome::Denied { .. } => FinishReason::Halted,
        };

        Ok(TurnOutcome {
            assistant_message_id: message_id,
            finish_reason: final_finish,
            usage,
            cost_usd: cost,
        })
    }

    fn turn_cost(&self, model: &str, usage: Option<&Usage>) -> Option<Decimal> {
        let usage = usage?;
        let pricing = self.provider.pricing(model)?;
        Some(compute_cost(usage, &pricing))
    }

    fn add_session_cost(&self, session_id: SessionId, cost: Decimal) {
        self.session_costs
            .entry(session_id)
            .and_modify(|v| *v += cost)
            .or_insert(cost);
    }

    /// Public additive cost recorder used by integrator plugins
    /// (typically a `CoreApi::record_cost` callback). Same DashMap
    /// update as [`Self::add_session_cost`] but exposed across crate
    /// boundaries.
    pub fn add_session_cost_external(&self, session_id: SessionId, delta: Decimal) {
        self.add_session_cost(session_id, delta);
    }

    async fn create_assistant_message(
        &self,
        session_id: SessionId,
    ) -> Result<MessageId, CoreError> {
        let msg = Message {
            id: MessageId::new(),
            session_id,
            role: Role::Assistant,
            created_at: Utc::now(),
        };
        let id = self.memory.append_message(session_id, msg).await?;
        self.events
            .publish(
                AgentEvent::MessageCreated {
                    session_id,
                    message_id: id,
                    at: Utc::now(),
                },
                Persistence::Durable,
            )
            .await?;
        Ok(id)
    }

    async fn publish_error(&self, session_id: SessionId, err: &CoreError) {
        let _ = self
            .events
            .publish(
                AgentEvent::Error {
                    session_id: Some(session_id),
                    code: err.class().as_str().to_string(),
                    message: err.to_string(),
                },
                Persistence::Durable,
            )
            .await;
    }
}
