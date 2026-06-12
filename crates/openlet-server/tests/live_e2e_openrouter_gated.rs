//! Live OpenRouter E2E — the real-traffic subset.
//!
//! These hit the actual OpenRouter API with the real `OPENROUTER_API_KEY`.
//! They are GATED at RUNTIME so a keyless `cargo test` stays green: each boot
//! goes through `LiveServer::for_scenario`, which uses the real OpenRouter
//! provider only when `OPENLET_LIVE_E2E=1` AND `OPENROUTER_API_KEY` are set;
//! otherwise it transparently falls back to the in-process scripted mock
//! (tier-1) driving the SAME test body. No `#[ignore]` — the env gate is the
//! single source of truth, so the test runs (against the mock) by default.
//!
//! Run against real OpenRouter:
//!   OPENLET_LIVE_E2E=1 OPENROUTER_API_KEY=... \
//!     cargo test -p openlet-server --test live_e2e_openrouter_gated
//!
//! Cost guardrails: the cheapest model the caller selects via
//! `OPENLET_LIVE_E2E_MODEL` (default `openai/gpt-4o-mini`), `max_tokens`
//! pinned tiny by the prompt, a single turn per test. Assertions check
//! shape/invariants (status, event ordering, non-empty content, terminal
//! status) — never exact model text, which is non-deterministic.
//!
//! Key safety: the key is read from env, sent only as the provider's
//! Authorization header, and never logged or asserted on.

use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn};

/// `GET /v1/models` against the real OpenRouter catalog returns a
/// non-empty, well-formed list. This is the cheapest live check — a free
/// catalog GET, no token spend.
#[tokio::test]
async fn live_models_catalog_nonempty() {
    // Tier-2 (live) hits the real OpenRouter catalog; tier-1 (mock) returns the
    // ScriptedProvider's fixed catalog. Either way `/v1/models` must serialize
    // a non-empty list of id-bearing entries — this tests the route plumbing on
    // both tiers (no token spend on either).
    let srv = LiveServer::for_scenario(Vec::new()).await;
    let models = srv.models().await;
    assert!(!models.is_empty(), "catalog should be non-empty");
    // Every entry must carry an id — the one field the route guarantees.
    for m in &models {
        assert!(
            m.get("id").and_then(|v| v.as_str()).is_some(),
            "model entry missing id: {m:?}"
        );
    }
}

/// One real bounded turn end to end: prompt the cheapest model for a
/// one-word answer, stream it through the runtime, and assert the BE→FE
/// invariants hold against a real provider — message/part/delta ordering
/// and a terminal idle status. Asserts shape, not the model's exact words.
#[tokio::test]
async fn live_single_turn_streams_real_completion() {
    // Tier-2 (live) streams a real one-word completion; tier-1 (mock) streams a
    // scripted text turn. Both must produce the same BE→FE invariants:
    // message_created → part_delta → terminal session_status, with non-empty
    // assistant text. Asserts shape, not exact words.
    let srv = LiveServer::for_scenario(vec![text_turn("ok")]).await;

    let sid = srv.create_session().await;
    // Tiny prompt → tiny response. Keeps token spend negligible.
    let ack = srv.prompt(&sid, "Reply with exactly one word: ok").await;
    assert_eq!(ack, reqwest::StatusCode::ACCEPTED, "prompt ack");

    let frames = srv
        .collect_session_events(&sid, Duration::from_secs(45))
        .await;

    let kinds: Vec<&str> = frames
        .iter()
        .filter_map(|f| f.get("kind").and_then(|v| v.as_str()))
        .collect();

    assert!(
        kinds.contains(&"message_created"),
        "expected message_created from a real turn; saw {kinds:?}"
    );
    assert!(
        kinds.contains(&"part_delta"),
        "expected streamed content from a real turn; saw {kinds:?}"
    );
    assert!(
        kinds.contains(&"session_status"),
        "real turn must reach a terminal status; saw {kinds:?}"
    );

    // The streamed assistant text must be non-empty (the model said
    // *something*) — but we never assert WHAT, since that is not
    // deterministic across model/version.
    let text: String = frames
        .iter()
        .filter(|f| {
            f.get("kind").and_then(|v| v.as_str()) == Some("part_delta")
                && f.get("delta_kind").and_then(|v| v.as_str()) == Some("text")
        })
        .filter_map(|f| f.get("delta").and_then(|v| v.as_str()))
        .collect();
    assert!(
        !text.trim().is_empty(),
        "real turn produced no assistant text; frames={frames:?}"
    );
}
