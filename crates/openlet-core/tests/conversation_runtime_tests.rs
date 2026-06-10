//! End-to-end tests for `ConversationRuntime` against in-memory mocks.
//!
//! Mocks (provider, memory, event sink) live at the top of the file; tests
//! at the bottom drive the orchestrator with canned `ChatDelta` streams and
//! assert on persisted parts + captured events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream;
use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::adapters::model_provider::{
    ChatDelta, ChatRequest, ChatStream, FinishReason, ModelPricing, ModelProvider,
};
use openlet_core::error::{CoreError, EventError, MemoryError, ProviderError};
use openlet_core::projection::{LlmMessage, LlmRole};
use openlet_core::runtime::{ConversationRuntime, RuntimeConfig, TurnInput};
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::{AgentEvent, DeltaKind, EventFilter, Usage};
use openlet_core::types::message::{Message, MessageId};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};

#[derive(Default)]
struct MockMemoryStore {
    parts: Mutex<HashMap<MessageId, Vec<Part>>>,
    messages: Mutex<Vec<(SessionId, Message)>>,
}

#[async_trait]
impl MemoryStore for MockMemoryStore {
    async fn create_session(
        &self,
        _: AgentId,
        _: Option<SessionId>,
    ) -> Result<SessionId, MemoryError> {
        Ok(SessionId::new())
    }
    async fn get_session(&self, _: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
        Ok(None)
    }
    async fn list_sessions(&self, _: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError> {
        Ok(vec![])
    }
    async fn update_status(
        &self,
        _: SessionId,
        _: SessionStatus,
        _: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn update_permission_mode(
        &self,
        _: SessionId,
        _: openlet_core::types::permission::PermissionMode,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn switch_agent(&self, _: SessionId, _: &str) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn update_session_extensions(
        &self,
        _: SessionId,
        _: serde_json::Value,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn delete_session(&self, _: SessionId) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn append_message(&self, sid: SessionId, msg: Message) -> Result<MessageId, MemoryError> {
        let id = msg.id;
        self.messages.lock().unwrap().push((sid, msg));
        Ok(id)
    }
    async fn append_part(&self, mid: MessageId, part: Part) -> Result<PartId, MemoryError> {
        let pid = part.id();
        self.parts
            .lock()
            .unwrap()
            .entry(mid)
            .or_default()
            .push(part);
        Ok(pid)
    }
    async fn upsert_part(
        &self,
        mid: MessageId,
        pid: PartId,
        part: Part,
    ) -> Result<(), MemoryError> {
        let mut g = self.parts.lock().unwrap();
        let v = g.entry(mid).or_default();
        if let Some(slot) = v.iter_mut().find(|p| p.id() == pid) {
            *slot = part;
        } else {
            v.push(part);
        }
        Ok(())
    }
    async fn list_messages(&self, sid: SessionId) -> Result<Vec<Message>, MemoryError> {
        Ok(self
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|(s, _)| *s == sid)
            .map(|(_, m)| m.clone())
            .collect())
    }
    async fn record_read(&self, _: SessionId, _: PathBuf) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn list_parts(&self, _: SessionId, msg: MessageId) -> Result<Vec<Part>, MemoryError> {
        Ok(self
            .parts
            .lock()
            .unwrap()
            .get(&msg)
            .cloned()
            .unwrap_or_default())
    }
}

struct MockEventSink {
    tx: broadcast::Sender<openlet_core::adapters::event_sink::DeliveredEvent>,
    captured: Mutex<Vec<(AgentEvent, Persistence)>>,
}

impl Default for MockEventSink {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            tx,
            captured: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl EventSink for MockEventSink {
    async fn publish(&self, ev: AgentEvent, p: Persistence) -> Result<(), EventError> {
        self.captured.lock().unwrap().push((ev.clone(), p));
        let _ = self
            .tx
            .send(openlet_core::adapters::event_sink::DeliveredEvent {
                event_id: None,
                event: ev,
            });
        Ok(())
    }
    fn subscribe(
        &self,
        _: EventFilter,
    ) -> broadcast::Receiver<openlet_core::adapters::event_sink::DeliveredEvent> {
        self.tx.subscribe()
    }
}

struct MockProvider {
    fixed: Mutex<Option<Vec<Result<ChatDelta, ProviderError>>>>,
    rx: Mutex<Option<mpsc::Receiver<Result<ChatDelta, ProviderError>>>>,
    pricing: Option<ModelPricing>,
}

impl MockProvider {
    fn fixed(events: Vec<Result<ChatDelta, ProviderError>>) -> Self {
        Self {
            fixed: Mutex::new(Some(events)),
            rx: Mutex::new(None),
            pricing: None,
        }
    }
    fn from_receiver(rx: mpsc::Receiver<Result<ChatDelta, ProviderError>>) -> Self {
        Self {
            fixed: Mutex::new(None),
            rx: Mutex::new(Some(rx)),
            pricing: None,
        }
    }
    fn with_pricing(mut self, pricing: ModelPricing) -> Self {
        self.pricing = Some(pricing);
        self
    }
}

#[async_trait]
impl ModelProvider for MockProvider {
    async fn chat_stream(
        &self,
        _: ChatRequest,
        _: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        if let Some(rx) = self.rx.lock().unwrap().take() {
            return Ok(Box::new(ReceiverStream::new(rx)));
        }
        let events = self.fixed.lock().unwrap().take().unwrap_or_default();
        Ok(Box::new(stream::iter(events)))
    }
    fn pricing(&self, _: &str) -> Option<ModelPricing> {
        self.pricing.clone()
    }
}

fn build(
    provider: MockProvider,
    cfg: RuntimeConfig,
) -> (
    Arc<ConversationRuntime>,
    Arc<MockMemoryStore>,
    Arc<MockEventSink>,
) {
    let memory = Arc::new(MockMemoryStore::default());
    let events = Arc::new(MockEventSink::default());
    let provider: Arc<dyn ModelProvider> = Arc::new(provider);
    let runtime = Arc::new(ConversationRuntime::new(
        provider,
        memory.clone() as Arc<dyn MemoryStore>,
        events.clone() as Arc<dyn EventSink>,
        cfg,
    ));
    (runtime, memory, events)
}

fn cfg() -> RuntimeConfig {
    RuntimeConfig::new("mock-model".into())
}

fn user_msg(text: &str) -> Vec<LlmMessage> {
    vec![LlmMessage {
        role: LlmRole::User,
        content: text.into(),
        reasoning: None,
        tool_calls: vec![],
        tool_call_id: None,
    }]
}

fn turn_input(session_id: SessionId) -> TurnInput {
    TurnInput {
        session_id,
        messages: user_msg("hi"),
        system_prompt: None,
        model: None,
        max_tokens: None,
        temperature: None,
        tools: vec![],
    }
}

#[tokio::test]
async fn streaming_text_persists_and_publishes_events() {
    let provider = MockProvider::fixed(vec![
        Ok(ChatDelta::Role),
        Ok(ChatDelta::Content {
            text: "Hello, ".into(),
        }),
        Ok(ChatDelta::Content {
            text: "world".into(),
        }),
        Ok(ChatDelta::Finish {
            reason: FinishReason::EndTurn,
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            }),
        }),
    ])
    .with_pricing(ModelPricing {
        input_per_mtok: Decimal::from_str("3.00").unwrap(),
        output_per_mtok: Decimal::from_str("15.00").unwrap(),
        cached_input_per_mtok: None,
        cache_write_per_mtok: None,
    });

    let session_id = SessionId::new();
    let (rt, memory, events) = build(provider, cfg());
    let outcome = rt
        .run_turn(turn_input(session_id), CancellationToken::new())
        .await
        .expect("turn ok");

    assert_eq!(outcome.finish_reason, FinishReason::EndTurn);
    assert!(outcome.cost_usd.is_some());
    assert_eq!(rt.session_cost(session_id), outcome.cost_usd.unwrap());

    let parts = memory
        .parts
        .lock()
        .unwrap()
        .get(&outcome.assistant_message_id)
        .cloned()
        .unwrap_or_default();
    let mut saw_text = false;
    let mut saw_finish = false;
    for p in &parts {
        match p {
            Part::Text { text, .. } if text == "Hello, world" => saw_text = true,
            Part::StepFinish { reason, .. } if reason == "end_turn" => saw_finish = true,
            _ => {}
        }
    }
    assert!(saw_text, "final text part not persisted: {parts:?}");
    assert!(saw_finish, "step_finish part not persisted");

    let captured: Vec<AgentEvent> = events
        .captured
        .lock()
        .unwrap()
        .iter()
        .map(|(e, _)| e.clone())
        .collect();
    assert!(matches!(
        captured.first(),
        Some(AgentEvent::MessageCreated { .. })
    ));
    assert!(captured.iter().any(|e| matches!(
        e,
        AgentEvent::PartDelta {
            delta_kind: DeltaKind::Text,
            ..
        }
    )));
    assert!(captured.iter().any(|e| matches!(
        e,
        AgentEvent::StepFinished {
            cost_decimal_str: Some(_),
            ..
        }
    )));
    let transient_count = events
        .captured
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, p)| *p == Persistence::Transient)
        .count();
    assert_eq!(transient_count, 2, "two text deltas should be transient");
}

#[tokio::test]
async fn tool_call_part_persisted_with_parsed_args() {
    let provider = MockProvider::fixed(vec![
        Ok(ChatDelta::ToolCallStart {
            call_id: "c1".into(),
            name: "bash".into(),
            index: 0,
        }),
        Ok(ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "{\"cmd\":\"ls\"}".into(),
        }),
        Ok(ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        }),
    ]);

    let session_id = SessionId::new();
    let (rt, memory, _) = build(provider, cfg());
    let outcome = rt
        .run_turn(turn_input(session_id), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(outcome.finish_reason, FinishReason::ToolUse);

    let parts = memory
        .parts
        .lock()
        .unwrap()
        .get(&outcome.assistant_message_id)
        .cloned()
        .unwrap_or_default();
    let tool = parts.iter().find_map(|p| match p {
        Part::ToolCall {
            call_id,
            name,
            args,
            ..
        } => Some((call_id.clone(), name.clone(), args.clone())),
        _ => None,
    });
    let (call_id, name, args) = tool.expect("tool_call part missing");
    assert_eq!(call_id, "c1");
    assert_eq!(name, "bash");
    assert_eq!(args["cmd"], "ls");
}

#[tokio::test]
async fn cancellation_returns_provider_cancelled() {
    let (tx, rx) = mpsc::channel(8);
    tx.send(Ok(ChatDelta::Content {
        text: "partial...".into(),
    }))
    .await
    .unwrap();
    let provider = MockProvider::from_receiver(rx);

    let session_id = SessionId::new();
    let (rt, _, events) = build(provider, cfg());
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_clone.cancel();
    });

    let err = rt
        .run_turn(turn_input(session_id), cancel)
        .await
        .expect_err("expected cancellation");
    assert!(
        matches!(err, CoreError::Provider(ProviderError::Cancelled)),
        "got {err:?}"
    );

    let saw_error = events.captured.lock().unwrap().iter().any(|(e, _)| {
        matches!(
            e,
            AgentEvent::Error {
                code,
                ..
            } if code == "provider_cancelled"
        )
    });
    assert!(saw_error, "Error event with provider_cancelled not emitted");
    drop(tx);
}

#[tokio::test]
async fn idle_timeout_surfaces_as_network_error() {
    let (tx, rx) = mpsc::channel::<Result<ChatDelta, ProviderError>>(1);
    let provider = MockProvider::from_receiver(rx);

    let session_id = SessionId::new();
    let mut c = cfg();
    c.idle_timeout = Duration::from_millis(50);
    let (rt, _, _) = build(provider, c);

    let err = rt
        .run_turn(turn_input(session_id), CancellationToken::new())
        .await
        .expect_err("expected timeout");
    assert!(
        matches!(err, CoreError::Provider(ProviderError::Network(ref m)) if m.contains("idle")),
        "got {err:?}"
    );
    drop(tx);
}

#[tokio::test]
async fn evict_session_cost_drops_the_entry() {
    // A turn that bills cost populates the session_costs map; eviction
    // (called on session DELETE) must remove it so the map can't grow
    // unbounded over a long-lived process.
    let provider = MockProvider::fixed(vec![
        Ok(ChatDelta::Content { text: "hi".into() }),
        Ok(ChatDelta::Finish {
            reason: FinishReason::EndTurn,
            usage: Some(Usage {
                input_tokens: 1000,
                output_tokens: 1000,
                ..Default::default()
            }),
        }),
    ])
    .with_pricing(ModelPricing {
        input_per_mtok: Decimal::from_str("3.00").unwrap(),
        output_per_mtok: Decimal::from_str("15.00").unwrap(),
        cached_input_per_mtok: None,
        cache_write_per_mtok: None,
    });
    let session_id = SessionId::new();
    let (rt, _memory, _events) = build(provider, cfg());
    rt.run_turn(turn_input(session_id), CancellationToken::new())
        .await
        .expect("turn ok");
    assert!(
        !rt.session_cost(session_id).is_zero(),
        "turn should have recorded cost"
    );

    rt.evict_session_cost(session_id);
    assert!(
        rt.session_cost(session_id).is_zero(),
        "evicted session must report zero cost"
    );
}

#[tokio::test]
async fn tool_args_deltas_stream_transiently_and_durable_toolcall_unchanged() {
    // Two args chunks split mid-JSON. They must surface as ordered
    // transient PartDelta{ToolArgs} events for the live frontend, while
    // the durable ToolCall part (flushed at Finish) carries the full
    // parsed args. The transient deltas are NOT persisted as parts.
    let provider = MockProvider::fixed(vec![
        Ok(ChatDelta::ToolCallStart {
            call_id: "c1".into(),
            name: "bash".into(),
            index: 0,
        }),
        Ok(ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "{\"cmd\":".into(),
        }),
        Ok(ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "\"ls\"}".into(),
        }),
        Ok(ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        }),
    ]);

    let session_id = SessionId::new();
    let (rt, memory, events) = build(provider, cfg());
    let outcome = rt
        .run_turn(turn_input(session_id), CancellationToken::new())
        .await
        .expect("turn ok");

    // Transient tool-args deltas observed in order.
    let captured = events.captured.lock().unwrap();
    let arg_deltas: Vec<(String, Persistence)> = captured
        .iter()
        .filter_map(|(e, p)| match e {
            AgentEvent::PartDelta {
                delta_kind: DeltaKind::ToolArgs,
                delta,
                ..
            } => Some((delta.clone(), *p)),
            _ => None,
        })
        .collect();
    assert_eq!(
        arg_deltas,
        vec![
            ("{\"cmd\":".to_string(), Persistence::Transient),
            ("\"ls\"}".to_string(), Persistence::Transient),
        ],
        "tool-args deltas must stream in order and be transient-only"
    );

    // The transient tool-args part_id must NOT have produced a durable
    // PartCreated (live-view-only; durable ToolCall is the record).
    let tool_args_delta_part_ids: std::collections::HashSet<PartId> = captured
        .iter()
        .filter_map(|(e, _)| match e {
            AgentEvent::PartDelta {
                delta_kind: DeltaKind::ToolArgs,
                part_id,
                ..
            } => Some(*part_id),
            _ => None,
        })
        .collect();
    for (e, _) in captured.iter() {
        if let AgentEvent::PartCreated { part_id, .. } = e {
            assert!(
                !tool_args_delta_part_ids.contains(part_id),
                "tool-args streaming part must never be persisted via PartCreated"
            );
        }
    }
    drop(captured);

    // Durable ToolCall part carries the full reassembled args.
    let parts = memory
        .parts
        .lock()
        .unwrap()
        .get(&outcome.assistant_message_id)
        .cloned()
        .unwrap_or_default();
    let args = parts.iter().find_map(|p| match p {
        Part::ToolCall { args, .. } => Some(args.clone()),
        _ => None,
    });
    let args = args.expect("durable tool_call part missing");
    assert_eq!(args["cmd"], "ls", "durable ToolCall args unchanged");
}
