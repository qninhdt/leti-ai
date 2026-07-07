//! Live OpenRouter E2E — authoritative `usage.cost` plumbing proof.
//!
//! Verifies the cost fix: OpenRouter returns `usage.cost` in the final
//! stream chunk (when `stream_options.include_usage` is set), and that value
//! must flow through `UsageWire.cost` → `Usage.cost_usd` → `turn_cost`
//! (preferred over the static pricing table) → the SSE `step_finished`
//! event's `cost_decimal_str`. Before the fix, `gemini-3.5-flash` had no
//! pricing-table row, so the displayed cost was always `$0.0000`.
//!
//! GATED at runtime like the rest of the live tier: the real provider is used
//! only when `OPENLET_LIVE_E2E=1` AND `OPENAI_API_KEY` are set; otherwise
//! the harness falls back to the scripted mock (no network, no `#[ignore]`).
//!
//! Run explicitly (forces gemini via the harness model env):
//!   OPENLET_LIVE_E2E=1 OPENAI_API_KEY=... \
//!     OPENLET_LIVE_E2E_MODEL=google/gemini-3.5-flash \
//!     cargo test -p openlet-server --test live_e2e_cost_usage -- --nocapture
//!
//! Host-safe: a pure-text greeting in the default `workspace_write` mode —
//! no tool calls, no bash, no file writes. Does NOT need the danger-mode
//! sandbox the tool-driving tests require.

use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn};

/// One real text turn; assert a `step_finished` frame carries a non-null,
/// parseable, non-negative `cost_decimal_str`. That field is `None` unless
/// the gateway's `usage.cost` was plumbed through — so a populated value is
/// the end-to-end proof of the fix for a model with no static pricing row.
#[tokio::test]
async fn real_turn_reports_gateway_cost() {
    // Tier-2 (live) proves the gateway's usage.cost flows through; tier-1
    // (mock) proves the same SSE plumbing with the ScriptedProvider's pricing +
    // usage. Both must surface a parseable non-negative cost_decimal_str on a
    // step_finished frame — that field is None unless cost was plumbed through.
    let srv = LiveServer::for_scenario(vec![text_turn("hello there friend")]).await;
    let sid = srv.create_session().await;

    // Pure text — no tools. Keep the spend tiny.
    srv.prompt(
        &sid,
        "Reply with exactly the three words: hello there friend",
    )
    .await;

    let frames = srv
        .collect_session_events(&sid, Duration::from_secs(60))
        .await;

    // Find the step_finished frame(s) and pull their cost_decimal_str.
    let costs: Vec<String> = frames
        .iter()
        .filter(|f| f.get("kind").and_then(|k| k.as_str()) == Some("step_finished"))
        .filter_map(|f| {
            f.get("cost_decimal_str")
                .and_then(|c| c.as_str())
                .map(str::to_owned)
        })
        .collect();

    assert!(
        !costs.is_empty(),
        "expected at least one step_finished frame carrying cost_decimal_str; \
         got frames: {frames:#?}"
    );

    // The authoritative gateway cost must parse and be non-negative. We do
    // NOT assert an exact figure (it depends on live token counts + billing),
    // only that the value is real — proving usage.cost was plumbed through
    // rather than dropped (which would have left it absent/None).
    let parsed: f64 = costs
        .last()
        .unwrap()
        .parse()
        .expect("cost_decimal_str must parse as a number");
    assert!(
        parsed >= 0.0,
        "gateway cost must be non-negative, got {parsed}"
    );

    eprintln!("observed gateway cost_decimal_str values: {costs:?}");
}
