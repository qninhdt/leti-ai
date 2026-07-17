//! Indexer reference custom agent. Stub-grade — proves the registration
//! flow. Real indexing is post-MVP per the brainstorm.

use std::sync::Arc;

use leti_core::agent::AgentDefinition;

use crate::builder::{AgentBlueprint, build};

const INDEXER_CACHEABLE: &str = "You are the Leti workspace indexer.\n\
For MVP this agent only logs and returns 'not yet implemented'.\n\
Real indexing of code symbols, embeddings, and references lands post-MVP.\n";

const TOOL_ALLOWLIST: &[&str] = &["read", "list", "glob"];

#[must_use]
pub fn indexer_agent() -> AgentDefinition {
    build(AgentBlueprint {
        slug: "indexer",
        title: "Workspace Indexer (stub)",
        description: "Reference custom agent — read-only, returns a stub response.",
        cacheable: INDEXER_CACHEABLE.to_owned(),
        dynamic: Arc::new(|_| String::new()),
        tool_allowlist: TOOL_ALLOWLIST,
    })
}
