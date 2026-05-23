//! Six adapter trait modules — the contracts implementations plug into.
//!
//! Phase 1 locks the surface; impls live in `openlet-adapters` (stubs only
//! this phase, real impls land in Phase 2-4).

pub mod artifact_store;
pub mod event_sink;
pub mod memory_store;
pub mod model_provider;
pub mod permission_manager;
pub mod tool_executor;

pub use artifact_store::{ArtifactRef, ArtifactStore};
pub use event_sink::{EventSink, Persistence};
pub use memory_store::MemoryStore;
pub use model_provider::{ChatDelta, ChatRequest, ModelPricing, ModelProvider};
pub use permission_manager::PermissionManager;
pub use tool_executor::{
    BashCommand, BashOutput, DirEntry, FileBlob, GrepArgs, GrepHit, ToolCtx, ToolExecutor,
};
