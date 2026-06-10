//! Tool layer — typed `Tool` trait, type-erased registry, read-history tracker.
//!
//! Tools declare a typed `Input`/`Output` pair and a `permission(input)`
//! mapping; the runtime erases each registered tool to `dyn ErasedTool`
//! and dispatches via `ToolRegistry::run(name, ctx, json)`. No
//! inventory/macro magic — registration is a manual `register_tools()`.

pub mod builtins;
pub mod dispatcher;
pub mod erased;
pub mod read_history;
pub mod registry;

pub use dispatcher::{ToolDispatchResult, ToolInvocation, dispatch_batch};
pub use erased::ErasedTool;
pub use read_history::ReadHistory;
pub use registry::{ToolRegistry, ToolRegistryBuilder};

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::types::permission::PermissionRequest;

/// Typed tool. Implementations declare a JSON-schema-able `Input`, a
/// serializable `Output`, the permission they require, and a `parallel_safe`
/// flag. The registry erases concrete types away at registration time.
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    /// Strongly typed input — must derive `JsonSchema` so the registry can
    /// hand a JSON schema to the model provider.
    type Input: DeserializeOwned + JsonSchema + Send + 'static;
    /// Strongly typed output. Serialized to JSON for the model and the
    /// projection layer.
    type Output: Serialize + Send + 'static;

    /// Stable wire name (e.g. `"read"`, `"bash"`). Must be unique within
    /// a registry.
    fn name(&self) -> &'static str;

    /// One-line description handed to the model alongside the schema.
    fn description(&self) -> &'static str;

    /// Whether this tool is safe to run in parallel with other safe
    /// tools in the same assistant turn. Defaults to `false` (serial).
    fn parallel_safe(&self) -> bool {
        false
    }

    /// Map a typed input to the permission(s) the runtime must check
    /// before invoking `run`. Phase-4 ruleset matcher takes the resulting
    /// `permission: String` plus `PermissionMode`.
    fn permission(&self, input: &Self::Input) -> PermissionRequest;

    /// Execute the tool. The runtime guarantees `permission` was checked
    /// (and any pending ask resolved) before this is called.
    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError>;
}

/// Convenience alias for the boxed-Arc form the registry stores.
pub type ToolHandle = Arc<dyn ErasedTool>;
