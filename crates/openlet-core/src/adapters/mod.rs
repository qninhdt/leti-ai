//! Six adapter trait modules — the contracts implementations plug into.
//!
//! These traits lock the surface; impls live in `openlet-adapters`.

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
pub use model_provider::{ChatDelta, ChatRequest, ModelInfo, ModelPricing, ModelProvider};
pub use permission_manager::PermissionManager;
pub use tool_executor::{
    BashCommand, BashOutput, DirEntry, FileBlob, GrepArgs, GrepHit, ToolCtx, ToolExecutor,
};
