//! Standalone runner for the mock OpenAI-compat service.
//!
//! Useful for manual smoke testing the TUI / server end-to-end without
//! burning real tokens.

use tokio::signal;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let svc = leti_test_mock_provider::MockOpenAiService::spawn().await?;
    eprintln!("mock-openai-service listening at {}", svc.base_url());
    eprintln!(
        "scenarios: simple_text | with_tool_call | reasoning | context_overflow | rate_limit | mid_stream_cancel"
    );
    eprintln!("usage: include `PARITY_SCENARIO:<name>` in the user message text");

    signal::ctrl_c().await?;
    eprintln!("\nshutdown requested; exiting.");
    Ok(())
}
