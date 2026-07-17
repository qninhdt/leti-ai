//! Tool registry — name-keyed dispatcher built once at boot.
//!
//! Built via `ToolRegistryBuilder::register::<T>()` (manual, NO inventory
//! per brainstorm §17). The builder validates name uniqueness before
//! handing back a frozen `Arc<ToolRegistry>`.

use std::collections::HashMap;
use std::sync::Arc;

use schemars::JsonSchema;
use serde_json::Value;

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::types::permission::PermissionRequest;

use super::{ErasedTool, Tool, ToolHandle};

/// Frozen, cloneable registry. Lookups are O(1).
pub struct ToolRegistry {
    tools: HashMap<&'static str, ToolHandle>,
}

impl ToolRegistry {
    #[must_use]
    pub fn builder() -> ToolRegistryBuilder {
        ToolRegistryBuilder::default()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<ToolHandle> {
        self.tools.get(name).cloned()
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tools.keys().copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &ToolHandle)> {
        self.tools.iter().map(|(k, v)| (*k, v))
    }

    /// Convenience — look up + dispatch. Returns `ToolError::NotFound` if
    /// the name isn't registered.
    pub async fn run(&self, name: &str, ctx: ToolCtx, input: Value) -> Result<Value, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        tool.run_json(ctx, input).await
    }

    /// Permission-request preview — used by the runtime to perform the
    /// permission check before dispatching `run`.
    pub fn permission(&self, name: &str, input: &Value) -> Result<PermissionRequest, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        tool.permission(input)
    }
}

#[derive(Default)]
pub struct ToolRegistryBuilder {
    tools: HashMap<&'static str, ToolHandle>,
}

impl ToolRegistryBuilder {
    /// Register a typed tool. Panics if the name is already taken — this
    /// is a boot-time invariant, not a runtime concern.
    #[must_use]
    pub fn register<T>(mut self, tool: T) -> Self
    where
        T: Tool,
        T::Input: JsonSchema,
    {
        let name = Tool::name(&tool);
        let handle: ToolHandle = Arc::new(tool);
        assert!(
            self.tools.insert(name, handle).is_none(),
            "duplicate tool name in registry: {name}"
        );
        self
    }

    /// Register an already-erased tool — useful for plugin tools that
    /// arrive as `Arc<dyn ErasedTool>` from a plugin manifest.
    #[must_use]
    pub fn register_erased(mut self, tool: ToolHandle) -> Self {
        let name = ErasedTool::name(tool.as_ref());
        assert!(
            self.tools.insert(name, tool).is_none(),
            "duplicate tool name in registry: {name}"
        );
        self
    }

    #[must_use]
    pub fn build(self) -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry { tools: self.tools })
    }
}
