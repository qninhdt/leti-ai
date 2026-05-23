//! Agent definitions — what an agent IS (prompt, allowlist, model).
//!
//! Distinct from `types::agent::AgentSpec` (boot-time identity + workspace).
//! An `AgentDefinition` is a *behavior* registered by a plugin during
//! `install`; an `AgentSpec` is an *identity* the runtime routes to.
//!
//! The split mirrors opencode's split between the agent registry (`agent.ts`
//! built-ins) and the per-session principal (`message.agent: string`).
//! claw-code has no agent abstraction, so the design comes entirely from
//! opencode adjusted for our compile-time plugin model.

mod definition;
mod registry;
mod slug;

pub use definition::{AgentDefinition, DynamicSegmentFn, DynamicSegmentInput, PromptSegments};
pub use registry::AgentRegistry;
pub use slug::{AgentSlug, SlugError};
