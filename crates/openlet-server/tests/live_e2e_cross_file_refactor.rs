//! Gated real-OpenRouter cross-file refactor E2E — a multi-tool, multi-turn
//! proof that a real model can DISCOVER call sites across a mini codebase and
//! REWRITE them consistently.
//!
//! This is harder than the FS-CRUD test: the model must (a) use `grep` to find
//! every occurrence of a symbol it was NOT handed the locations of, (b) open
//! and `edit` each file, and (c) leave the tree internally consistent. The
//! deterministic mock cannot do this — only a real model chains grep-result →
//! per-file edit → next file.
//!
//! Gated identically to the other live tiers: the runtime env gate
//! (`OPENLET_LIVE_E2E=1` + `OPENROUTER_API_KEY`) selects the real provider;
//! unset, the harness falls back to the scripted mock so `cargo test` makes no
//! network calls.
//!
//! Run against real OpenRouter:
//!   OPENLET_LIVE_E2E=1 OPENROUTER_API_KEY=... \
//!     cargo test -p openlet-server --test live_e2e_cross_file_refactor
//!
//! Zero mocks: real `LocalFilesystem` + `SqliteMemoryStore` +
//! `ConfigPermissionMgr` (Danger mode). Assertions read on-disk state and are
//! tolerant of the model's narration — they check the refactor INVARIANT
//! (old symbol fully gone, new symbol present in every file), not exact text.

use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn, tool_turn};

/// Poll a predicate over the workspace until true or the deadline passes.
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

/// A real model is asked to rename a function used across three files. It is
/// NOT told which files or lines — it must `grep` to discover them, then `edit`
/// each. The assertion is the refactor invariant on disk: the old name appears
/// in ZERO files, the new name appears in all three.
///
/// Two-tier: tier-2 (live) lets a real model discover + rename; tier-1 (mock)
/// scripts grep→edit×3→grep. Both dispatch the real grep/edit tools against the
/// real fs, so the on-disk refactor invariant is meaningful on either tier.
#[tokio::test]
async fn real_model_renames_symbol_across_files() {
    // Tier-1 script: discover with grep, rename each file with edit
    // (replace_all since report.py/main.py carry `get_total` twice — import +
    // call site), verify with a final grep. The edits carry the REAL rename and
    // run on both tiers, so this drives the same wiring it asserts.
    let script = vec![
        tool_turn("g1", "grep", r#"{"pattern":"get_total"}"#),
        tool_turn(
            "e1",
            "edit",
            r#"{"path":"calc.py","find":"get_total","replace":"compute_sum","replace_all":true}"#,
        ),
        tool_turn(
            "e2",
            "edit",
            r#"{"path":"report.py","find":"get_total","replace":"compute_sum","replace_all":true}"#,
        ),
        tool_turn(
            "e3",
            "edit",
            r#"{"path":"main.py","find":"get_total","replace":"compute_sum","replace_all":true}"#,
        ),
        tool_turn("g2", "grep", r#"{"pattern":"get_total"}"#),
        text_turn("DONE"),
    ];
    let srv = LiveServer::for_scenario(script).await;
    let ws = srv.workspace_root().to_path_buf();

    // Seed a tiny codebase: one definition + two call sites of `get_total`.
    // The old name is deliberately distinctive so grep finds it cleanly and
    // the assertion can't false-match a substring.
    std::fs::write(
        ws.join("calc.py"),
        "def get_total(items):\n    return sum(items)\n",
    )
    .expect("seed calc.py");
    std::fs::write(
        ws.join("report.py"),
        "from calc import get_total\n\ndef summary(items):\n    return f\"total={get_total(items)}\"\n",
    )
    .expect("seed report.py");
    std::fs::write(
        ws.join("main.py"),
        "from calc import get_total\n\nprint(get_total([1, 2, 3]))\n",
    )
    .expect("seed main.py");

    let sid = srv.create_session().await;
    assert_eq!(
        srv.set_mode(&sid, "danger").await,
        reqwest::StatusCode::OK,
        "set danger mode"
    );

    let prompt = "There is a Python function named `get_total` that is defined \
        in one file and called in others in the working directory. Rename it to \
        `compute_sum` EVERYWHERE. Steps: \
        1) use the grep tool to find every file that contains `get_total`. \
        2) for EACH such file, use the edit tool to replace `get_total` with \
        `compute_sum` (this includes the definition, the imports, and every \
        call site). \
        3) when every occurrence is renamed, run the grep tool once more for \
        `get_total` to confirm zero matches remain, then reply DONE. \
        Do not create new files; only edit the existing ones.";
    srv.prompt(&sid, prompt).await;

    // Cross-file discovery + 3 edits + a verifying grep is a long multi-turn
    // run; give it a generous bounded budget.
    let _frames = srv
        .collect_session_events(&sid, Duration::from_secs(120))
        .await;

    let read = |name: &str| std::fs::read_to_string(ws.join(name)).unwrap_or_default();

    // Invariant 1: the old symbol is gone from EVERY file.
    let old_gone = wait_disk(
        || {
            !read("calc.py").contains("get_total")
                && !read("report.py").contains("get_total")
                && !read("main.py").contains("get_total")
        },
        Duration::from_secs(8),
    )
    .await;
    assert!(
        old_gone,
        "old symbol `get_total` must not remain in any file.\n\
         calc.py:\n{}\nreport.py:\n{}\nmain.py:\n{}",
        read("calc.py"),
        read("report.py"),
        read("main.py"),
    );

    // Invariant 2: the new symbol is present in all three files (definition +
    // both importers/callers), proving the rename was applied, not just deleted.
    assert!(
        read("calc.py").contains("compute_sum"),
        "definition file must use the new name: {}",
        read("calc.py")
    );
    assert!(
        read("report.py").contains("compute_sum"),
        "caller report.py must use the new name: {}",
        read("report.py")
    );
    assert!(
        read("main.py").contains("compute_sum"),
        "caller main.py must use the new name: {}",
        read("main.py")
    );

    // Invariant 3: no stray files were created (the model was told to edit in
    // place). Only the three seeds should exist.
    let py_files = std::fs::read_dir(&ws)
        .expect("read ws")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "py"))
        .count();
    assert_eq!(
        py_files, 3,
        "exactly the three seeded .py files should exist"
    );
}
