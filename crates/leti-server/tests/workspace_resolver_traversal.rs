//! Extended `workspace_data_root` traversal coverage.
//!
//! Source-side tests already cover the basics. This file extends:
//! - Every ASCII control char (0x00–0x1F, 0x7F) → Invalid
//! - 129-byte id (1 over cap) → Invalid; 128-byte id → Ok
//! - UTF-8 emoji id → allowed (no `/` or `\` in NFC form)
//! - URL-encoded `..` (`%2e%2e`) → passes through as a literal id
//!   (no decode happens in the resolver)

use std::path::PathBuf;

use leti_server::workspace_resolver::{WorkspaceError, workspace_data_root};

#[test]
fn rejects_every_ascii_control_char() {
    let base = PathBuf::from("/tmp");
    for code in 0u32..=0x1F {
        let id = format!("ws{}", char::from_u32(code).unwrap());
        let res = workspace_data_root(&base, &id);
        assert!(
            matches!(res, Err(WorkspaceError::Invalid(_))),
            "control char 0x{code:02X} must be rejected; got {res:?}"
        );
    }
    // DEL (0x7F)
    let id = format!("ws{}", char::from_u32(0x7F).unwrap());
    assert!(matches!(
        workspace_data_root(&base, &id),
        Err(WorkspaceError::Invalid(_))
    ));
}

#[test]
fn length_cap_at_128_bytes() {
    let base = PathBuf::from("/tmp");
    let exact = "a".repeat(128);
    assert!(
        workspace_data_root(&base, &exact).is_ok(),
        "128-byte id must be accepted (cap is inclusive at 128)"
    );

    let over = "a".repeat(129);
    assert!(
        matches!(
            workspace_data_root(&base, &over),
            Err(WorkspaceError::Invalid(_))
        ),
        "129-byte id must be rejected"
    );
}

#[test]
fn utf8_emoji_id_is_accepted() {
    let base = PathBuf::from("/tmp");
    // Emoji is multi-byte UTF-8 but contains no `/` or `\` and is
    // not a control character. Resolver must accept.
    let id = "workspace-\u{1F4BB}";
    let p = workspace_data_root(&base, id).expect("emoji id allowed");
    assert!(
        p.to_string_lossy().contains(id),
        "emoji must round-trip: {p:?}"
    );
}

#[test]
fn url_encoded_dotdot_passes_through_as_literal() {
    // `%2e%2e` is not decoded by the resolver — it's a literal id.
    // The resulting path does not contain `..`, so it can't escape
    // the workspaces dir. Lock that contract: percent-encoded
    // traversal markers are SAFE without a decode step.
    let base = PathBuf::from("/tmp");
    let id = "%2e%2e";
    let p = workspace_data_root(&base, id).expect("literal id allowed");
    assert!(
        p.ends_with("workspaces/%2e%2e"),
        "expected literal id to survive into path: {p:?}"
    );
}
