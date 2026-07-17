//! SQLite-backed adapters: `MemoryStore`, event repo, permission repo.
//!
//! `pool` exposes the connection helper + embedded migrations; `memory_store`
//! is the `MemoryStore` impl backed by sqlx.

pub(crate) mod codec;
pub mod event_repo;
pub(crate) mod memory_queries;
pub mod memory_store;
pub mod permission_repo;
pub mod pool;
pub(crate) mod rows;

pub use memory_store::SqliteMemoryStore;
pub use pool::{MIGRATOR, open_in_memory, open_pool, run_migrations};
