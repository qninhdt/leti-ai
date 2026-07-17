//! Prompt cache hash lock.
//!
//! Silently editing the cacheable segment invalidates the Anthropic prompt
//! cache for every active session. This test makes such an edit a
//! deliberate change: the BLAKE3 hash below MUST be updated alongside the
//! `# version: N` header in `general_cacheable.md` whenever the prompt
//! body changes intentionally.

use leti_plugin_core_agents::GENERAL_CACHEABLE;

/// BLAKE3 hash of the cacheable general-agent prompt at version 2.
///
/// To bump: update `# version: N` in `general_cacheable.md` AND replace
/// this constant with the new hash. The mismatch makes drift visible.
const GENERAL_PROMPT_CACHE_LOCK: &str =
    "c929a6529a7143892c8283972537c3a0c422a3d7f969907eec63799a22db4b63";

#[test]
fn general_prompt_cacheable_hash_locked() {
    let actual = blake3::hash(GENERAL_CACHEABLE.as_bytes())
        .to_hex()
        .to_string();
    assert_eq!(
        actual, GENERAL_PROMPT_CACHE_LOCK,
        "general agent cacheable prompt drifted; bump `# version:` header AND \
         update GENERAL_PROMPT_CACHE_LOCK to {actual}"
    );
}

#[test]
fn version_header_present() {
    assert!(
        GENERAL_CACHEABLE.contains("# version:"),
        "cacheable prompt missing `# version: N` header"
    );
}
