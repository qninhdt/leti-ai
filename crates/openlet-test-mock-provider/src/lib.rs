//! In-process OpenAI-compat mock server for parity / replay testing.
//!
//! Uses a hand-rolled `tokio::net::TcpListener` (NOT axum) so tests have
//! byte-exact control over chunking, headers, and timing — properties
//! a higher-level framework would hide.
//!
//! Scenarios are a hard-coded `Scenario` enum; each emits either an
//! SSE byte stream (status 200) or a non-streaming JSON error
//! (status >= 400). Selection: scan the inbound `messages[].content`
//! for `PARITY_SCENARIO:<name>` and parse the suffix. If no token
//! is present, the server returns `simple_text`.
//!
//! Public API:
//! - [`MockOpenAiService::spawn`] — bind to `127.0.0.1:0` and serve
//! - [`MockOpenAiService::base_url`] — feed into `OpenAiCompatProvider::new`
//! - [`MockOpenAiService::captured_requests`] — assert what arrived
//! - [`SCENARIO_PREFIX`] — public so tests can build user-message text
//!
//! Drop kills the accept task and any in-flight connections.

mod scenarios;
mod server;

pub use scenarios::{FS_WRITE_CONTENT, FS_WRITE_PATH, SCENARIO_PREFIX, Scenario, detect_scenario};
pub use server::{CapturedRequest, MockOpenAiService};
