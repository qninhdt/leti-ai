//! Gated real-OpenRouter plan-mode E2E — the profile-swap state-machine proof.
//!
//! `enter_plan_mode` / `exit_plan_mode` are the only path a model has to flip
//! the session's active agent profile: enter switches to the read-only `plan`
//! profile and emits `PlanModeEntered`; exit carries the model's frozen plan
//! text, restores the prior profile, emits `PlanModeExited`, and persists a
//! durable `Part::Plan` for audit/replay. This test drives a real model
//! through enter→(produce plan)→exit and asserts the persisted artifact — only
//! a model that actually called both tools in sequence produces it.
//!
//! Gated identically to the other live tiers: the runtime env gate
//! (`OPENLET_LIVE_E2E=1` + `OPENAI_API_KEY`) selects the real provider;
//! unset, the harness falls back to the scripted mock so `cargo test` makes no
//! network calls.
//!
//! Run against real OpenRouter:
//!   OPENLET_LIVE_E2E=1 OPENAI_API_KEY=... \
//!     cargo test -p openlet-server --test live_e2e_plan_mode

use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn, tool_turn};

use openlet_core::types::part::Part;
use openlet_core::types::session::SessionId;

/// Scan persisted parts for a `Part::Plan`, returning its text. The plan is
/// durable (persisted by `exit_plan_mode`), so this reads straight from the
/// same sqlite the server writes — race-free vs the transient SSE stream.
async fn persisted_plan_text(srv: &LiveServer, sid: &str) -> Option<String> {
    let session = SessionId(sid.parse().expect("uuid"));
    let memory = srv.memory();
    let messages = memory.list_messages(session).await.unwrap_or_default();
    for msg in messages {
        let parts = memory.list_parts(session, msg.id).await.unwrap_or_default();
        for p in parts {
            if let Part::Plan { plan, .. } = p {
                return Some(plan);
            }
        }
    }
    None
}

/// Tell the model to enter plan mode, draft a short plan containing a
/// distinctive sentinel, then exit plan mode submitting that plan. Assert a
/// durable `Part::Plan` was persisted carrying the sentinel — proving the full
/// enter→exit state machine ran and froze the plan for operator review.
///
/// Two-tier: tier-2 (live) lets a real model run enter→draft→exit; tier-1
/// (mock) scripts the enter_plan_mode + exit_plan_mode calls. Both dispatch the
/// real plan-mode tools (profile swap + durable Part::Plan persist + events),
/// so the persisted-plan + transition-event assertions hold on either tier.
const PLAN_SENTINEL: &str = "PLAN_SENTINEL_7731";

#[tokio::test]
async fn real_model_enters_and_exits_plan_mode() {
    // Tier-1 script: enter plan mode, then exit submitting a plan carrying the
    // sentinel. The exit_plan_mode tool persists the Part::Plan + fires events
    // on both tiers, so this is not a tautology — it drives the same wiring.
    let exit_args = format!(r#"{{"plan":"1. scaffold\n2. {PLAN_SENTINEL}\n3. ship"}}"#);
    let script = vec![
        tool_turn("p1", "enter_plan_mode", "{}"),
        tool_turn("p2", "exit_plan_mode", &exit_args),
        text_turn("DONE"),
    ];
    let srv = LiveServer::for_scenario(script).await;
    let sid = srv.create_session().await;
    // Danger mode so the `agent:enter_plan_mode` / `agent:exit_plan_mode`
    // permission checks auto-allow instead of parking on an Ask (no human is
    // here to approve them).
    assert_eq!(
        srv.set_mode(&sid, "danger").await,
        reqwest::StatusCode::OK,
        "set danger mode"
    );

    // Distinctive sentinel the model must carry into the plan text, so the
    // assertion can't false-match generic prose.
    let sentinel = PLAN_SENTINEL;
    let prompt = format!(
        "First call the enter_plan_mode tool. Then draft a SHORT three-step \
         plan for building a TODO app; the plan text MUST include the exact \
         token `{sentinel}` somewhere. Then call the exit_plan_mode tool, \
         passing that full plan text as the `plan` argument. After exiting, \
         reply DONE. Do not skip either tool call."
    );
    let ack = srv.prompt(&sid, &prompt).await;
    assert_eq!(ack, reqwest::StatusCode::ACCEPTED, "prompt ack");

    // enter → model drafts → exit is a multi-step tool sequence; bounded.
    let frames = srv
        .collect_session_events(&sid, Duration::from_secs(120))
        .await;

    // Primary proof: a durable Part::Plan was persisted carrying the sentinel.
    let mut plan = persisted_plan_text(&srv, &sid).await;
    // Small grace for the final persist to settle after terminal status.
    if plan.is_none() {
        tokio::time::sleep(Duration::from_secs(2)).await;
        plan = persisted_plan_text(&srv, &sid).await;
    }
    let plan = plan.expect("exit_plan_mode must persist a durable Part::Plan");
    assert!(
        plan.contains(sentinel),
        "the persisted plan must carry the sentinel the model was told to \
         include (proves the model's plan text, not an empty/synthetic part). \
         Plan was: {plan:?}"
    );

    // Corroborate via the event stream: both plan-mode transitions fired.
    // EventDto uses snake_case kinds (`plan_mode_entered`/`plan_mode_exited`).
    let kinds: Vec<String> = frames
        .iter()
        .filter_map(|f| {
            f.get("kind")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(
        kinds.iter().any(|k| k == "plan_mode_entered"),
        "expected a plan_mode_entered event; saw {kinds:?}"
    );
    assert!(
        kinds.iter().any(|k| k == "plan_mode_exited"),
        "expected a plan_mode_exited event; saw {kinds:?}"
    );
}
