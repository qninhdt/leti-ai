//! Agent definitions — what an agent IS (prompt, allowlist, model).
//!
//! Distinct from `types::agent::AgentSpec` (boot-time identity + workspace).
//! An `AgentDefinition` is a *behavior* registered by a plugin during
//! `install`; an `AgentSpec` is an *identity* the runtime routes to.
//!
//! The split separates the agent registry (built-in behaviors)
//! from the per-session principal the runtime routes to.

mod definition;
mod registry;
mod slug;

pub use definition::{AgentDefinition, DynamicSegmentFn, DynamicSegmentInput, PromptSegments};
pub use registry::AgentRegistry;
pub use slug::{AgentSlug, SlugError};
