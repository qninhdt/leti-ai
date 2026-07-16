//! Type-erased dispatcher for `Tool`.
//!
//! `Tool` is generic; storage demands a single dyn-compatible trait.
//! `ErasedTool::run_json` parses `serde_json::Value` into `T::Input`,
//! invokes `T::run`, and serializes the output back to JSON. Schema
//! generation lives behind a separate method so the registry can build
//! `ToolSpec`s for the model provider without instantiating a tool call.

use async_trait::async_trait;
use schemars::JsonSchema;
use schemars::schema_for;
use serde_json::Value;

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::types::permission::PermissionRequest;

use super::{CancellationPolicy, PromptPolicy, Tool, ToolConcurrency};

/// Object-safe shadow of `Tool`. Stored as `Arc<dyn ErasedTool>` in the
/// registry. Errors from JSON (de)serialization map to
/// `ToolError::InvalidInput` / `ToolError::Io` so callers see a stable
/// `ToolError` regardless of the underlying tool.
#[async_trait]
pub trait ErasedTool: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parallel_safe(&self) -> bool;
    /// Parse input and classify it before admission. This is deliberately
    /// separate from `run_json`: malformed input is terminal and never takes
    /// a permit or a resource lock.
    fn concurrency(&self, _input: &Value) -> Result<ToolConcurrency, ToolError> {
        #[allow(deprecated)]
        Ok(if self.parallel_safe() {
            ToolConcurrency::concurrent()
        } else {
            ToolConcurrency::exclusive()
        })
    }
    fn prompt_policy(&self) -> PromptPolicy {
        PromptPolicy::Interactive
    }
    fn cancellation_policy(&self) -> CancellationPolicy {
        CancellationPolicy::AbortSafe
    }

    /// JSON Schema for the tool's input (`schemars`-generated). Returned
    /// as a `serde_json::Value` so callers can splice it directly into a
    /// provider request without an extra serde round-trip.
    fn input_schema(&self) -> Value;

    /// Map a raw input JSON to the permission request. Errors if the
    /// JSON does not conform to the typed input.
    fn permission(&self, input: &Value) -> Result<PermissionRequest, ToolError>;

    /// Execute the tool with a JSON input and return a JSON output.
    async fn run_json(&self, ctx: ToolCtx, input: Value) -> Result<Value, ToolError>;
}

#[async_trait]
impl<T: Tool> ErasedTool for T
where
    T::Input: JsonSchema,
{
    fn name(&self) -> &'static str {
        Tool::name(self)
    }
    fn description(&self) -> &'static str {
        Tool::description(self)
    }
    fn parallel_safe(&self) -> bool {
        #[allow(deprecated)]
        Tool::parallel_safe(self)
    }
    fn concurrency(&self, input: &Value) -> Result<ToolConcurrency, ToolError> {
        let typed: T::Input = serde_json::from_value(input.clone())
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        Ok(Tool::concurrency(self, &typed))
    }
    fn prompt_policy(&self) -> PromptPolicy {
        Tool::prompt_policy(self)
    }
    fn cancellation_policy(&self) -> CancellationPolicy {
        Tool::cancellation_policy(self)
    }

    fn input_schema(&self) -> Value {
        let schema = schema_for!(T::Input);
        serde_json::to_value(schema).unwrap_or_else(|e| {
            // A schemars-generated schema is always serializable, so this
            // path should be unreachable. If it ever fires, surface it
            // loudly rather than silently shipping a null schema (which the
            // provider would reject or mis-handle as "no input").
            tracing::error!(
                tool = Tool::name(self),
                error = %e,
                "tool input schema failed to serialize; sending empty object schema"
            );
            serde_json::json!({ "type": "object" })
        })
    }

    fn permission(&self, input: &Value) -> Result<PermissionRequest, ToolError> {
        let typed: T::Input = serde_json::from_value(input.clone())
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        Ok(Tool::permission(self, &typed))
    }

    async fn run_json(&self, ctx: ToolCtx, input: Value) -> Result<Value, ToolError> {
        let typed: T::Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let out = Tool::run(self, ctx, typed).await?;
        serde_json::to_value(out).map_err(|e| ToolError::Io(e.to_string()))
    }
}
