//! `make_pool()` — short alias for `open_in_memory()` with migrations
//! applied. Promotes the boilerplate that already lives in every
//! adapters test that touches SQLite.

use openlet_adapters::sqlite::open_in_memory;
use sqlx::SqlitePool;

/// Fresh in-memory SQLite pool with migrations applied. Each call
/// returns an isolated database — caller owns it for the duration of
/// the test.
pub async fn make_pool() -> SqlitePool {
    open_in_memory()
        .await
        .expect("open_in_memory: failed to bring up :memory: SQLite pool")
}
