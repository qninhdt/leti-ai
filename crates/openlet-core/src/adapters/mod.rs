//! Six adapter trait modules — the contracts implementations plug into.
//!
//! Phase 1 locks the surface; impls live in `openlet-adapters` (stubs only
//! this phase, real impls land in Phase 2-4).

pub mod artifact_store;
pub mod event_sink;
pub mod filesystem;
pub mod hooked_event_sink;
pub mod hooked_memory_store;
pub mod memory_store;
pub mod model_provider;
pub mod permission_manager;
pub mod tool_executor;

pub use artifact_store::{ArtifactRef, ArtifactStore};
pub use event_sink::{EventSink, Persistence};
pub use filesystem::{
    ByteRange, DirEntry as FsDirEntry, FileMeta, Filesystem, GlobOpts, GlobSort,
    GrepArgs as FsGrepArgs, GrepHit as FsGrepHit, WriteOpts,
};
pub use memory_store::MemoryStore;
pub use model_provider::{ChatDelta, ChatRequest, ModelPricing, ModelProvider};
pub use permission_manager::PermissionManager;
pub use tool_executor::{
    BashCommand, BashOutput, DirEntry, FileBlob, GrepArgs, GrepHit, ToolCtx, ToolExecutor,
};
