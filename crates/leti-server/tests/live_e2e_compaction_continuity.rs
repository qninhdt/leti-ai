//! Gated real-OpenRouter compaction-continuity E2E — the hardest live proof:
//! that a long conversation crosses the compaction threshold, the runtime
//! ACTUALLY compacts (a real summarization turn against the model), and a fact
//! planted BEFORE compaction is still recalled AFTER it.
//!
//! Why this needs real infrastructure: compaction only runs when
//! `loop_ctx.agent` is `Some` and the projected token count exceeds
//! `context_window * compaction_threshold`. The default harness wires an empty
//! agent registry (→ `None` → compaction never fires), so this test boots via
//! `with_openrouter_small_window`, which registers a `general` agent with a
//! deliberately tiny context window so a handful of turns trip the threshold.
//! The summarization step itself is a real model turn — only a live model can
//! produce a summary that preserves the planted fact.
//!
//! Gated identically to the other live tiers: the runtime env gate
//! (`LETI_LIVE_E2E=1` + `OPENAI_API_KEY`) selects the real provider;
//! unset, the harness falls back to the scripted mock so `cargo test` makes no
//! network calls.
//!
//! Run against real OpenRouter:
//!   LETI_LIVE_E2E=1 OPENAI_API_KEY=... \
//!     cargo test -p leti-server --test live_e2e_compaction_continuity

use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn};

use leti_core::types::part::Part;
use leti_core::types::session::SessionId;

/// Scan every persisted part of the session for a `Part::Compaction`, proving
/// the runtime genuinely compacted (not merely that the model rambled). Reads
/// straight from the same sqlite the server writes — compaction parts are
/// durable, unlike transient `part.delta` frames.
async fn compaction_fired(srv: &LiveServer, sid: &str) -> bool {
    let session = SessionId(sid.parse().expect("uuid"));
    let memory = srv.memory();
    let messages = memory.list_messages(session).await.unwrap_or_default();
    for msg in messages {
        let parts = memory.list_parts(session, msg.id).await.unwrap_or_default();
        if parts.iter().any(|p| matches!(p, Part::Compaction { .. })) {
            return true;
        }
    }
    false
}

/// Read the LAST assistant message's text straight from the persisted parts
/// table (lowercased). The final assistant text lives in `Part::Text`, not the
/// transient SSE `part_delta` stream — reading memory is race-free even when a
/// compaction-slowed turn outruns the SSE collect window. Returns the text of
/// the most recent message that has any `Part::Text`, so it captures the recall
/// answer rather than an earlier acknowledgment.
async fn last_assistant_text_lower(srv: &LiveServer, sid: &str) -> String {
    let session = SessionId(sid.parse().expect("uuid"));
    let memory = srv.memory();
    let messages = memory.list_messages(session).await.unwrap_or_default();
    let mut latest = String::new();
    for msg in messages {
        let parts = memory.list_parts(session, msg.id).await.unwrap_or_default();
        let text: String = parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        if !text.trim().is_empty() {
            latest = text;
        }
    }
    latest.to_lowercase()
}

/// Plant a distinctive fact, bury it under several turns of unrelated chatter
/// (each turn re-projects the whole history, so the token count climbs into
/// the compaction threshold of the small-window agent), then ask the model to
/// recall the fact. Assert BOTH: compaction actually fired, and the fact
/// survived into the post-compaction answer.
///
/// Two-tier: tier-1 (mock) proves compaction FIRES (durable Part::Compaction)
/// and the post-compaction answer is plumbed through — its scripted turns all
/// carry the sentinel (incl. the drained-queue fallback, since compaction
/// inserts extra summarization turns), so recall is preserved by construction.
/// tier-2 (live) additionally proves a REAL model recalls the fact across a
/// real summarization turn. Same body, both meaningful.
#[tokio::test]
async fn fact_survives_real_compaction() {
    // Tier-1: only the first turn (plant ack) is scripted; EVERY subsequent
    // turn — fillers, the compaction summarization turns (unpredictable count),
    // and the final recall — falls through to the sentinel-bearing fallback.
    // This removes turn-alignment fragility (compaction inserts extra model
    // calls), and the assertion still meaningfully checks compaction FIRED and
    // the answer plumbed through. Recall-correctness is tier-2's job.
    let sentinel_line = "the project codename is GREENFINCH-42";
    let script = vec![text_turn(&format!("noted: {sentinel_line}"))];
    // Very small window so even a couple of short turns trip
    // `context_window * 0.5` (= 50 tokens ≈ 200 chars).
    let srv = LiveServer::for_scenario_small_window(
        100,
        script,
        Some("The codename is GREENFINCH-42.".to_string()),
    )
    .await;
    let sid = srv.create_session().await;

    // Turn 1: plant the fact. Distinctive token so recall can't be a lucky
    // guess and the assertion can't false-match.
    let secret = "the project codename is GREENFINCH-42";
    srv.prompt(
        &sid,
        &format!(
            "Remember this fact for later, it is important: {secret}. \
             Just acknowledge you've noted it in one short sentence."
        ),
    )
    .await;
    srv.collect_session_events(&sid, Duration::from_secs(60))
        .await;

    // Turns 2..N: unrelated filler to grow the projected history past the
    // compaction threshold. Each is a full turn (history re-projected), so the
    // token count climbs. Kept short + cheap.
    let fillers = [
        "Briefly, what is 2 + 2?",
        "Name one primary color in one word.",
        "Say the word 'ok' and nothing else.",
        "What day comes after Monday? One word.",
        "Reply with a single short sentence about water.",
    ];
    for f in fillers {
        srv.prompt(&sid, f).await;
        srv.collect_session_events(&sid, Duration::from_secs(60))
            .await;
    }

    // Compaction should have fired by now (threshold crossed on a later turn).
    assert!(
        compaction_fired(&srv, &sid).await,
        "expected a durable Part::Compaction — the small-window agent should \
         have crossed the compaction threshold across {} turns",
        fillers.len() + 1
    );

    // Final turn: ask for the planted fact. If compaction dropped/garbled it,
    // the model can't answer correctly — this is the continuity proof.
    srv.prompt(
        &sid,
        "What was the project codename I told you to remember earlier? \
         Answer with the exact codename.",
    )
    .await;
    // Drain the answer turn to terminal status, then read the persisted answer
    // from memory (race-free vs the SSE stream under a compaction-slowed turn).
    srv.collect_session_events(&sid, Duration::from_secs(120))
        .await;

    let answer = last_assistant_text_lower(&srv, &sid).await;
    assert!(
        answer.contains("greenfinch-42") || answer.contains("greenfinch"),
        "the planted codename must survive compaction and be recalled. \
         Assistant said: {answer:?}"
    );
}
