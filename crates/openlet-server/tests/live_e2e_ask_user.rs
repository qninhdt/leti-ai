//! Gated real-OpenRouter ask_user E2E — the human-in-the-loop proof.
//!
//! `ask_user` is the most complex multi-actor flow in the runtime: the model
//! calls the tool, the turn PARKS on a rendezvous oneshot, a durable
//! `question.requested` event hits the SSE stream, the client POSTs an answer
//! to `/v1/sessions/:id/question/answer`, and only then does the parked tool
//! resume and feed the selection back into the model's next turn. Nothing
//! short of a real model exercises the full arc — the model must (a) decide to
//! ask, (b) wait, then (c) act on the answer it could not have predicted.
//!
//! The default `POST /v1/session` route ships headless-safe (capabilities
//! `{}` → `user_questions=false`), so this test boots a question-capable
//! session via the harness helper. Gated identically to the other live tiers
//! (`#[ignore]` + `OPENLET_LIVE_E2E=1` + `OPENROUTER_API_KEY`).
//!
//! Run:
//!   OPENLET_LIVE_E2E=1 cargo test -p openlet-server --test \
//!     live_e2e_ask_user -- --ignored

use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn, tool_turn};

async fn wait_disk(pred: impl Fn() -> bool, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    pred()
}

/// The model is told to ASK the user to choose a color, then act on the answer
/// by writing the chosen color to a file. The test plays the human: it watches
/// the SSE stream for the parked `question.requested`, answers with a specific
/// option index, and then asserts the model wrote the option WE selected — the
/// end-to-end proof that the answer routed back into the model's reasoning.
///
/// Two-tier: tier-2 (live) lets a real model ask then act; tier-1 (mock)
/// scripts ask_user→write. On BOTH tiers the ask_user tool genuinely parks the
/// turn on the rendezvous, the test answers concurrently, and the resumed turn
/// writes the choice — so the park→answer→resume wiring + the single-use 404
/// replay guard are exercised identically.
#[tokio::test]
async fn real_model_asks_user_then_acts_on_the_answer() {
    // Tier-1 script: ask_user (options red=0, blue=1) parks the turn; after the
    // test answers [1], the runtime feeds the selection back and the scripted
    // write records "blue". The real ask_user + write tools run on both tiers.
    let ask_args = r#"{"header":"color","question":"Pick a color","options":[{"label":"red"},{"label":"blue"}]}"#;
    let script = vec![
        tool_turn("q1", "ask_user", ask_args),
        tool_turn("w1", "write", r#"{"path":"choice.txt","content":"blue\n"}"#),
        text_turn("DONE"),
    ];
    let srv = LiveServer::for_scenario(script).await;
    let ws = srv.workspace_root().to_path_buf();
    let choice_file = ws.join("choice.txt");

    // Question-capable session (opt-in; default sessions are headless-safe).
    let sid = srv.create_question_capable_session().await;
    // Danger mode so the post-answer `write` auto-allows against the real
    // permission manager.
    assert_eq!(
        srv.set_mode(&sid, "danger").await,
        reqwest::StatusCode::OK,
        "set danger mode"
    );

    // Options are ordered so index 1 == "blue". We will answer with [1] and
    // then assert the model wrote "blue" — proving it used OUR selection, not
    // a default or the first option.
    let prompt = "Use the ask_user tool to ask me to choose a favorite color. \
        Provide EXACTLY two options in this order: first option label `red`, \
        second option label `blue`. After I answer, write my chosen color (the \
        label of the option I selected) into a file named `choice.txt` in the \
        working directory, then reply DONE. Do not guess my choice before I \
        answer; wait for the ask_user result.";
    let ack = srv.prompt(&sid, prompt).await;
    assert_eq!(ack, reqwest::StatusCode::ACCEPTED, "prompt ack");

    // Play the human: wait for the parked question, then answer "blue" (idx 1).
    let qid = srv
        .wait_for_question(&sid, Duration::from_secs(60))
        .await
        .expect("model should emit a question.requested and park on it");

    let answer_status = srv.answer_question(&sid, &qid, vec![1]).await;
    assert_eq!(
        answer_status,
        reqwest::StatusCode::OK,
        "answering the pending question must be accepted"
    );

    // Drain the resumed turn to terminal (the model now acts on the answer).
    let _frames = srv
        .collect_session_events(&sid, Duration::from_secs(90))
        .await;

    // The end-to-end proof: the model wrote the option WE chose. If the answer
    // hadn't routed back into the model's context, it couldn't know "blue".
    let wrote = wait_disk(
        || {
            std::fs::read_to_string(&choice_file)
                .map(|s| s.to_lowercase().contains("blue"))
                .unwrap_or(false)
        },
        Duration::from_secs(8),
    )
    .await;
    assert!(
        wrote,
        "model must write the user-selected color 'blue' to choice.txt; \
         contents: {:?}",
        std::fs::read_to_string(&choice_file).ok()
    );

    // Negative guard: it must NOT have written the unselected option as the
    // choice. (Tolerant: only fail if 'red' appears and 'blue' does not — the
    // file could legitimately mention both in a sentence.)
    let body = std::fs::read_to_string(&choice_file).unwrap_or_default();
    let lower = body.to_lowercase();
    assert!(
        lower.contains("blue"),
        "choice file must record the selected option 'blue', got: {body:?}"
    );

    // A second answer to the same (now-resolved) question must 404 —
    // single-use rendezvous semantics, proven live.
    let replay = srv.answer_question(&sid, &qid, vec![0]).await;
    assert_eq!(
        replay,
        reqwest::StatusCode::NOT_FOUND,
        "a replayed answer to an already-resolved question must 404"
    );
}
