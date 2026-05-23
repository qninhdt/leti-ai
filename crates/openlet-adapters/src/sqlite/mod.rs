//! SQLite-backed adapters: `MemoryStore`, event repo, permission repo.
//!
//! `pool` exposes the connection helper + embedded migrations; `memory_store`
//! is the `MemoryStore` impl backed by sqlx.

pub mod event_repo;
pub mod memory_store;
pub mod permission_repo;
pub mod pool;

pub use memory_store::SqliteMemoryStore;
pub use pool::{open_in_memory, open_pool, run_migrations, MIGRATOR};
