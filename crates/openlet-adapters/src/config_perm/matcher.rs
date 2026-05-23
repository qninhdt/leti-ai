//! Subject extraction — given a tool input JSON, build the
//! `permission` string the ruleset matcher receives.
//!
//! Mirrors claw-code `permissions.rs:447-469` (`extract_permission_subject`):
//! tries a small ordered list of well-known JSON keys; falls back to
//! the verb-only string. The verb is the tool name (`read`, `bash`,
//! `edit`, …); the target depends on the tool's input shape.

use serde_json::Value;

/// Ordered list of input keys we accept as the permission target.
/// Matches the tool input shapes in [`builtins`](super::super) — `path`
/// for filesystem tools, `command` for bash, `pattern` for glob/grep.
const TARGET_KEYS: &[&str] = &[
    "command",
    "path",
    "file_path",
    "filePath",
    "url",
    "pattern",
    "query",
];

/// Build a `permission` string for matcher consumption.
///
/// `tool_name` becomes the verb; the subject is extracted from `input`.
/// If no known key is present, the verb is returned bare (matches the
/// `Any`-rule case in claw-code).
#[must_use]
pub fn build_permission_subject(tool_name: &str, input: &Value) -> String {
    let target = input.as_object().and_then(|obj| {
        TARGET_KEYS.iter().find_map(|k| {
            obj.get(*k)
                .and_then(Value::as_str)
                .map(std::string::ToString::to_string)
        })
    });
    match target {
        Some(t) if !t.is_empty() => format!("{tool_name}:{t}"),
        _ => tool_name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_path() {
        let s = build_permission_subject("read", &json!({"path": "src/main.rs"}));
        assert_eq!(s, "read:src/main.rs");
    }

    #[test]
    fn extracts_command_first() {
        let s = build_permission_subject(
            "bash",
            &json!({"command": "git status", "path": "ignored"}),
        );
        assert_eq!(s, "bash:git status");
    }

    #[test]
    fn falls_back_to_verb() {
        let s = build_permission_subject("todo", &json!({"todos": []}));
        assert_eq!(s, "todo");
    }
}
