//! Gated real-OpenRouter scaffold→discover→summarize E2E — proves a real model
//! can BUILD a small multi-directory project, then turn around and use the
//! read-only discovery tools (`glob`, `list`, `grep`) to inventory what it
//! built and write a manifest derived from that discovery.
//!
//! This is the "explore an unfamiliar tree" workflow: the model writes several
//! files into subdirectories, then must find them again with glob/grep (NOT
//! from memory of the paths it chose) and synthesize a summary file. The
//! deterministic mock can't chain create → discover → summarize.
//!
//! Gated identically to the other live tiers (`#[ignore]` +
//! `OPENLET_LIVE_E2E=1` + `OPENROUTER_API_KEY`).
//!
//! Run:
//!   OPENLET_LIVE_E2E=1 cargo test -p openlet-server --test \
//!     live_e2e_scaffold_discovery -- --ignored
//!
//! Zero mocks: real `LocalFilesystem` (subdir creation, glob, grep) +
//! permission mgr in Danger mode. Assertions read on-disk structure and the
//! generated manifest; tolerant of the model's wording.

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

/// The model scaffolds three module files under `src/` (each exposing a
/// distinctively-named function), then must DISCOVER them via glob/grep and
/// write `MANIFEST.txt` listing the function names it found. The assertion
/// checks: the files landed in the subdir, AND the manifest names the three
/// functions — proving the discovery half actually read the tree back.
///
/// Two-tier: tier-2 (live) lets a real model scaffold + discover + summarize;
/// tier-1 (mock) scripts write×3→glob→grep→write-manifest. Both dispatch the
/// real write/glob/grep tools, so the on-disk structure + manifest assertions
/// are meaningful on either tier.
#[tokio::test]
async fn real_model_scaffolds_then_discovers_and_summarizes() {
    // Tier-1 script: scaffold three modules under src/, glob + grep to
    // "discover" them, then write the manifest. The writes carry the real
    // content the assertions check; all tools execute on both tiers.
    let script = vec![
        tool_turn(
            "w1",
            "write",
            r#"{"path":"src/alpha.py","content":"def handle_alpha():\n    return 'alpha'\n"}"#,
        ),
        tool_turn(
            "w2",
            "write",
            r#"{"path":"src/beta.py","content":"def handle_beta():\n    return 'beta'\n"}"#,
        ),
        tool_turn(
            "w3",
            "write",
            r#"{"path":"src/gamma.py","content":"def handle_gamma():\n    return 'gamma'\n"}"#,
        ),
        tool_turn("gl", "glob", r#"{"pattern":"src/*.py"}"#),
        tool_turn("gr", "grep", r#"{"pattern":"def handle_"}"#),
        tool_turn(
            "wm",
            "write",
            r#"{"path":"MANIFEST.txt","content":"handle_alpha\nhandle_beta\nhandle_gamma\n"}"#,
        ),
        text_turn("DONE"),
    ];
    let srv = LiveServer::for_scenario(script).await;
    let ws = srv.workspace_root().to_path_buf();

    let sid = srv.create_session().await;
    assert_eq!(
        srv.set_mode(&sid, "danger").await,
        reqwest::StatusCode::OK,
        "set danger mode"
    );

    // Distinctive function names so grep/the manifest can't false-match. The
    // model picks the paths under src/ itself; we only fix the names.
    let prompt = "Build a small Python project, then inventory it. Do these in \
        order, one tool call at a time: \
        1) create `src/alpha.py` defining a function `def handle_alpha():` that \
        returns the string 'alpha'. \
        2) create `src/beta.py` defining `def handle_beta():` returning 'beta'. \
        3) create `src/gamma.py` defining `def handle_gamma():` returning \
        'gamma'. \
        4) now FORGET the paths: use the glob tool (pattern `src/*.py`) and the \
        grep tool (search for `def handle_`) to discover every module and the \
        functions they define. \
        5) write a file `MANIFEST.txt` in the working-directory ROOT (not in \
        src/) containing one line per discovered function name: \
        `handle_alpha`, `handle_beta`, `handle_gamma` (one per line). \
        When MANIFEST.txt is written, reply DONE.";
    srv.prompt(&sid, prompt).await;

    // Scaffolding 3 files + glob + grep + manifest write is a long run.
    let _frames = srv
        .collect_session_events(&sid, Duration::from_secs(150))
        .await;

    let alpha = ws.join("src").join("alpha.py");
    let beta = ws.join("src").join("beta.py");
    let gamma = ws.join("src").join("gamma.py");
    let manifest = ws.join("MANIFEST.txt");

    // Invariant 1: the three modules were created under the src/ subdirectory.
    let scaffolded = wait_disk(
        || alpha.exists() && beta.exists() && gamma.exists(),
        Duration::from_secs(8),
    )
    .await;
    assert!(
        scaffolded,
        "expected src/alpha.py, src/beta.py, src/gamma.py to exist"
    );

    // Each module defines its distinctively-named function (proves real
    // content, not empty files).
    assert!(
        std::fs::read_to_string(&alpha)
            .unwrap_or_default()
            .contains("handle_alpha"),
        "alpha.py must define handle_alpha"
    );
    assert!(
        std::fs::read_to_string(&gamma)
            .unwrap_or_default()
            .contains("handle_gamma"),
        "gamma.py must define handle_gamma"
    );

    // Invariant 2 — the discovery proof: the manifest at the root names all
    // three discovered functions. A model that skipped glob/grep and wrote a
    // wrong/partial manifest fails here.
    let manifest_present = wait_disk(|| manifest.exists(), Duration::from_secs(8)).await;
    assert!(manifest_present, "MANIFEST.txt must be written at the root");

    let body = std::fs::read_to_string(&manifest).unwrap_or_default();
    for func in ["handle_alpha", "handle_beta", "handle_gamma"] {
        assert!(
            body.contains(func),
            "manifest must list discovered function `{func}`. Manifest:\n{body}"
        );
    }
}
