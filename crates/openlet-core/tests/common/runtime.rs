//! `RuntimeFixture` — boot a `ConversationRuntime` wired to the in-memory
//! mocks (`ScriptedProvider`, `MockMemoryStore`, `RecordingEventSink`).
//!
//! Used by phase-2 end-to-end tests that exercise `run_turn` without
//! pulling in SQLite or a real model.

use std::sync::Arc;

use openlet_core::adapters::EventSink;
use openlet_core::adapters::MemoryStore;
use openlet_core::adapters::ModelProvider;
use openlet_core::runtime::{ConversationRuntime, RuntimeConfig};

use super::mock_event_sink::RecordingEventSink;
use super::mock_memory::MockMemoryStore;
use super::mock_provider::ScriptedProvider;

/// All four handles the test author commonly needs: typed concrete handles
/// for setup (`provider.push_text_turn`, `events.snapshot()`) plus the
/// `runtime` itself for driving turns.
pub struct RuntimeFixture {
    pub provider: Arc<ScriptedProvider>,
    pub memory: Arc<MockMemoryStore>,
    pub events: Arc<RecordingEventSink>,
    pub runtime: ConversationRuntime,
}

impl RuntimeFixture {
    /// Boot with all defaults. Model defaults to `"test-model"` so any
    /// pricing lookup misses cleanly.
    #[must_use]
    pub fn boot() -> Self {
        Self::boot_with_model("test-model")
    }

    /// Boot with a specific default model name.
    #[must_use]
    pub fn boot_with_model(model: &str) -> Self {
        let provider = Arc::new(ScriptedProvider::new());
        let memory = Arc::new(MockMemoryStore::new());
        let events = Arc::new(RecordingEventSink::new());

        let provider_dyn: Arc<dyn ModelProvider> = provider.clone();
        let memory_dyn: Arc<dyn MemoryStore> = memory.clone();
        let events_dyn: Arc<dyn EventSink> = events.clone();

        let runtime = ConversationRuntime::new(
            provider_dyn,
            memory_dyn,
            events_dyn,
            RuntimeConfig::new(model.to_string()),
        );

        Self {
            provider,
            memory,
            events,
            runtime,
        }
    }
}
