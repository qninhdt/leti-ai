//! Tool mocks — `noop`, `failing`, `slow`, `panicking`. All implement
//! `ErasedTool` directly (skipping the typed `Tool` trait) so each
//! variant can return an arbitrary JSON value without a schemars input
//! dance. The registry stores them via `register_erased`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use leti_core::adapters::tool_executor::ToolCtx;
use leti_core::error::ToolError;
use leti_core::tools::{ErasedTool, ToolHandle, ToolRegistry};
use leti_core::types::permission::PermissionRequest;
use serde_json::{Value, json};

/// Tool that always succeeds; returns `{"call_id": "..."}` so callers
/// can verify the order of returned outcomes.
pub struct NoopTool {
    name: &'static str,
    parallel_safe: bool,
    runs: Arc<AtomicUsize>,
}

impl NoopTool {
    #[must_use]
    pub fn new(name: &'static str, parallel_safe: bool) -> Self {
        Self {
            name,
            parallel_safe,
            runs: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn run_count(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ErasedTool for NoopTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        "test noop tool"
    }
    fn parallel_safe(&self) -> bool {
        self.parallel_safe
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn permission(&self, _input: &Value) -> Result<PermissionRequest, ToolError> {
        Ok(PermissionRequest {
            permission: format!("test:{}", self.name),
            reason: None,
            timeout: None,
        })
    }
    async fn run_json(&self, ctx: ToolCtx, _input: Value) -> Result<Value, ToolError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"call_id": ctx.call_id}))
    }
}

/// Tool that always returns the supplied error. Cloning the error per
/// call keeps `ToolError`'s lack of `Clone` from spreading.
pub struct FailingTool {
    name: &'static str,
    err: fn() -> ToolError,
}

impl FailingTool {
    #[must_use]
    pub fn new(name: &'static str, err: fn() -> ToolError) -> Self {
        Self { name, err }
    }
}

#[async_trait]
impl ErasedTool for FailingTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        "test failing tool"
    }
    fn parallel_safe(&self) -> bool {
        false
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn permission(&self, _input: &Value) -> Result<PermissionRequest, ToolError> {
        Ok(PermissionRequest {
            permission: format!("test:{}", self.name),
            reason: None,
            timeout: None,
        })
    }
    async fn run_json(&self, _ctx: ToolCtx, _input: Value) -> Result<Value, ToolError> {
        Err((self.err)())
    }
}

/// Tool that sleeps for `delay_ms` then returns `{"call_id": "..."}`.
/// Used by parallel-order tests to widen the window where order
/// preservation could fail.
pub struct SlowTool {
    name: &'static str,
    delay_ms: u64,
    parallel_safe: bool,
}

impl SlowTool {
    #[must_use]
    pub fn new(name: &'static str, delay_ms: u64, parallel_safe: bool) -> Self {
        Self {
            name,
            delay_ms,
            parallel_safe,
        }
    }
}

#[async_trait]
impl ErasedTool for SlowTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        "test slow tool"
    }
    fn parallel_safe(&self) -> bool {
        self.parallel_safe
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn permission(&self, _input: &Value) -> Result<PermissionRequest, ToolError> {
        Ok(PermissionRequest {
            permission: format!("test:{}", self.name),
            reason: None,
            timeout: None,
        })
    }
    async fn run_json(&self, ctx: ToolCtx, _input: Value) -> Result<Value, ToolError> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(json!({"call_id": ctx.call_id}))
    }
}

/// Tool that panics from inside `run_json`. The dispatcher wraps tool
/// futures in `tokio::spawn` so the panic surfaces as
/// `ToolError::Io("tool 'X' panicked")` instead of unwinding the whole
/// runtime.
pub struct PanickingTool {
    name: &'static str,
}

impl PanickingTool {
    #[must_use]
    pub fn new(name: &'static str) -> Self {
        Self { name }
    }
}

#[async_trait]
impl ErasedTool for PanickingTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        "test panicking tool"
    }
    fn parallel_safe(&self) -> bool {
        true
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn permission(&self, _input: &Value) -> Result<PermissionRequest, ToolError> {
        Ok(PermissionRequest {
            permission: format!("test:{}", self.name),
            reason: None,
            timeout: None,
        })
    }
    async fn run_json(&self, _ctx: ToolCtx, _input: Value) -> Result<Value, ToolError> {
        panic!("intentional panic in test tool '{}'", self.name);
    }
}

/// Build a registry from a list of erased tool handles. Saves the
/// caller from chaining `.register_erased(...)` builder calls in tests.
#[must_use]
pub fn make_registry(tools: Vec<ToolHandle>) -> Arc<ToolRegistry> {
    let mut b = ToolRegistry::builder();
    for t in tools {
        b = b.register_erased(t);
    }
    b.build()
}
