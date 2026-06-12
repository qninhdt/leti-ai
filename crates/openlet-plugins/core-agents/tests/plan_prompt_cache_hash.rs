//! Prompt cache hash lock for the plan-mode agent.
//!
//! Silently editing the cacheable segment invalidates the Anthropic prompt
//! cache for every active session. This test makes such an edit a
//! deliberate change: the BLAKE3 hash below MUST be updated alongside the
//! `# version: N` header in `plan_mode.md` whenever the prompt body
//! changes intentionally.

use openlet_plugin_core_agents::PLAN_CACHEABLE;

/// BLAKE3 hash of the cacheable plan-mode prompt at version 1.
///
/// To bump: update `# version: N` in `plan_mode.md` AND replace
/// this constant with the new hash. The mismatch makes drift visible.
const PLAN_PROMPT_CACHE_LOCK: &str =
    "15267bd714d05ca9b15356555ad793a6bfc803711267b4cba62283d3759ba6be";

#[test]
fn plan_prompt_cacheable_hash_locked() {
    let actual = blake3::hash(PLAN_CACHEABLE.as_bytes()).to_hex().to_string();
    assert_eq!(
        actual, PLAN_PROMPT_CACHE_LOCK,
        "plan-mode agent cacheable prompt drifted; bump `# version:` header AND \
         update PLAN_PROMPT_CACHE_LOCK to {actual}"
    );
}

#[test]
fn plan_version_header_present() {
    assert!(
        PLAN_CACHEABLE.contains("# version:"),
        "plan-mode cacheable prompt missing `# version: N` header"
    );
}
