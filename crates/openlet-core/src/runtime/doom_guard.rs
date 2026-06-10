//! Doom-loop guard — pure stateless detector.
//!
//! Triggers on N consecutive turns where each turn:
//!   - has zero text output (assistant said nothing user-facing)
//!   - has zero successful tool writes (no `ToolResult { ok: true }`)
//!   - emits the SAME tool-call signature set OR a strict subset of the
//!     prior turn's set (handles agents narrowing tool calls each turn)
//!
//! Signature = `(tool_name, blake3(canonical_json(args)))`. This is
//! order-sensitive and brittle.
//!
//! Threshold = 3.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Threshold of consecutive matching turns required to abort.
pub const DEFAULT_THRESHOLD: usize = 3;

/// One tool-call signature in a turn. `arg_hash` is `blake3(canonical_json)`
/// of the parsed arguments. Stored hex for cheap equality + serde.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ToolCallSig {
    pub name: String,
    pub arg_hash: String,
}

impl ToolCallSig {
    /// Build a signature from a tool name + parsed JSON args.
    #[must_use]
    pub fn new(name: impl Into<String>, args: &serde_json::Value) -> Self {
        Self {
            name: name.into(),
            arg_hash: hash_args(args),
        }
    }
}

/// Canonical-JSON encode (sorted keys, no extra whitespace) → blake3 → hex.
fn hash_args(value: &serde_json::Value) -> String {
    let canonical = canonicalize(value);
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let hash = blake3::hash(&bytes);
    hash.to_hex().to_string()
}

fn canonicalize(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, serde_json::Value> =
                std::collections::BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), canonicalize(v));
            }
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize).collect())
        }
        other => other.clone(),
    }
}

/// One turn's observable behaviour for the guard.
#[derive(Debug, Clone, Default)]
pub struct TurnSummary {
    pub had_text_output: bool,
    pub had_successful_writes: bool,
    pub tool_calls: BTreeSet<ToolCallSig>,
}

/// Outcome of a doom check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoomVerdict {
    /// Loop is healthy; continue.
    Ok,
    /// Last `threshold` turns matched the abort criteria.
    Abort {
        /// Synthetic message to surface to the user.
        message: String,
    },
}

/// Pure check. `history` is newest-LAST. Returns `Abort` only when the last
/// `threshold` turns each had no text + no successful writes AND each turn's
/// tool-call set is equal-to or a strict subset of the prior turn's.
#[must_use]
pub fn check(history: &[TurnSummary], threshold: usize) -> DoomVerdict {
    if threshold == 0 || history.len() < threshold {
        return DoomVerdict::Ok;
    }

    let recent = &history[history.len() - threshold..];

    // Empty tool-call sets are not interesting — text-only turns can't loop.
    if recent.iter().any(|t| t.tool_calls.is_empty()) {
        return DoomVerdict::Ok;
    }
    if recent.iter().any(|t| t.had_text_output) {
        return DoomVerdict::Ok;
    }
    if recent.iter().any(|t| t.had_successful_writes) {
        return DoomVerdict::Ok;
    }

    // Each successive turn must be == or a strict subset of the previous.
    for window in recent.windows(2) {
        let prev = &window[0].tool_calls;
        let cur = &window[1].tool_calls;
        if !cur.is_subset(prev) {
            return DoomVerdict::Ok;
        }
    }

    DoomVerdict::Abort {
        message: "Detected repeated tool calls — aborting to prevent loop. \
                  Please refine your request."
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_THRESHOLD, DoomVerdict, ToolCallSig, TurnSummary, check};
    use std::collections::BTreeSet;

    fn turn(sigs: &[(&str, &str)]) -> TurnSummary {
        let mut set = BTreeSet::new();
        for (name, args) in sigs {
            let v: serde_json::Value = serde_json::from_str(args).unwrap();
            set.insert(ToolCallSig::new(*name, &v));
        }
        TurnSummary {
            had_text_output: false,
            had_successful_writes: false,
            tool_calls: set,
        }
    }

    #[test]
    fn aborts_on_three_identical_turns() {
        let h = vec![
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
        ];
        assert!(matches!(
            check(&h, DEFAULT_THRESHOLD),
            DoomVerdict::Abort { .. }
        ));
    }

    #[test]
    fn aborts_on_strict_subset_narrowing() {
        let h = vec![
            turn(&[("bash", r#"{"cmd":"a"}"#), ("bash", r#"{"cmd":"b"}"#)]),
            turn(&[("bash", r#"{"cmd":"a"}"#), ("bash", r#"{"cmd":"b"}"#)]),
            turn(&[("bash", r#"{"cmd":"a"}"#)]),
        ];
        assert!(matches!(
            check(&h, DEFAULT_THRESHOLD),
            DoomVerdict::Abort { .. }
        ));
    }

    #[test]
    fn ok_when_args_differ() {
        let h = vec![
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
            turn(&[("bash", r#"{"cmd":"pwd"}"#)]),
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
        ];
        assert_eq!(check(&h, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    #[test]
    fn ok_when_text_output_present() {
        let mut t = turn(&[("bash", r#"{"cmd":"ls"}"#)]);
        t.had_text_output = true;
        let h = vec![t.clone(), t.clone(), t];
        assert_eq!(check(&h, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    #[test]
    fn ok_when_writes_succeeded() {
        let mut t = turn(&[("bash", r#"{"cmd":"ls"}"#)]);
        t.had_successful_writes = true;
        let h = vec![t.clone(), t.clone(), t];
        assert_eq!(check(&h, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    #[test]
    fn ok_when_history_too_short() {
        let h = vec![
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
        ];
        assert_eq!(check(&h, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    #[test]
    fn ok_when_set_grows_across_turns() {
        let h = vec![
            turn(&[("bash", r#"{"cmd":"a"}"#)]),
            turn(&[("bash", r#"{"cmd":"a"}"#), ("bash", r#"{"cmd":"b"}"#)]),
            turn(&[
                ("bash", r#"{"cmd":"a"}"#),
                ("bash", r#"{"cmd":"b"}"#),
                ("bash", r#"{"cmd":"c"}"#),
            ]),
        ];
        assert_eq!(check(&h, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    #[test]
    fn canonical_json_order_invariant() {
        let a: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        let sa = ToolCallSig::new("t", &a);
        let sb = ToolCallSig::new("t", &b);
        assert_eq!(sa, sb);
    }

    #[test]
    fn canonical_json_preserves_array_order() {
        // Object key order is canonicalized; array order is NOT.
        // Lock that contract — different array orders MUST produce
        // different hashes, otherwise reordered tool-call lists would
        // falsely trigger the doom guard.
        let a: serde_json::Value = serde_json::from_str(r#"{"a":[1,2]}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{"a":[2,1]}"#).unwrap();
        let sa = ToolCallSig::new("t", &a);
        let sb = ToolCallSig::new("t", &b);
        assert_ne!(sa, sb, "array order must affect hash");
    }

    #[test]
    fn threshold_zero_always_returns_ok() {
        // Sanity: threshold of 0 is the "no guarding" mode; the guard
        // must early-exit and never abort.
        let h = vec![
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
            turn(&[("bash", r#"{"cmd":"ls"}"#)]),
        ];
        assert_eq!(check(&h, 0), DoomVerdict::Ok);
    }

    #[test]
    fn ok_when_strict_superset_grows() {
        // turn 3 has MORE tool calls than turn 2: cur is NOT a subset
        // of prev → Ok. Lock the direction of the subset check
        // (cur ⊆ prev), not (prev ⊆ cur).
        let h = vec![
            turn(&[("bash", r#"{"cmd":"a"}"#)]),
            turn(&[("bash", r#"{"cmd":"a"}"#), ("bash", r#"{"cmd":"b"}"#)]),
            turn(&[
                ("bash", r#"{"cmd":"a"}"#),
                ("bash", r#"{"cmd":"b"}"#),
                ("bash", r#"{"cmd":"c"}"#),
            ]),
        ];
        assert_eq!(check(&h, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }
}
