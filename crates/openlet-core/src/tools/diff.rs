//! Structured file diff emitted by the `edit`/`write` tools.
//!
//! The tool's typed `Output` is serialized to JSON and stored verbatim in
//! `Part::ToolResult.text` (see `runtime/turn_loop_helpers.rs`), then
//! parsed back by the TUI for rendering. Attaching a `FileDiff` to the
//! output therefore rides through SSE + persistence with no protocol
//! change — the client parses the same JSON body it already receives.
//!
//! The diff is line-level (Myers, via `similar::TextDiff::from_lines`) and
//! grouped into hunks with a few lines of surrounding context, mirroring a
//! unified diff. Output is capped: a bounded number of changed lines ships
//! over the wire; anything larger sets `truncated` so the UI can say so
//! rather than streaming a whole-file blob.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

/// Number of unchanged context lines kept around each changed block, the
/// same default `git diff` uses.
const CONTEXT_LINES: usize = 3;

/// Upper bound on the number of `DiffLine`s emitted across all hunks. A
/// diff exceeding this is truncated (with `truncated = true`) so a huge
/// edit never ships a multi-megabyte SSE frame.
pub const DEFAULT_LINE_CAP: usize = 400;

/// Kind of a single diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    /// Line present only in the new content.
    Add,
    /// Line present only in the old content.
    Del,
    /// Unchanged context line present in both.
    Ctx,
}

/// One line of a diff hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Line text with the trailing newline stripped.
    pub text: String,
}

/// A contiguous run of changes plus surrounding context — one unified-diff
/// hunk. `old_start`/`new_start` are 1-based line numbers of the hunk's
/// first line in each side (0 when that side is empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}

/// Structured line-level diff attached to an edit/write result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileDiff {
    /// Count of added lines across the whole diff (before truncation cap).
    pub added: usize,
    /// Count of removed lines across the whole diff (before truncation cap).
    pub removed: usize,
    /// Hunks, in file order. Empty when old == new.
    pub hunks: Vec<DiffHunk>,
    /// True when the diff exceeded the line cap and hunks were trimmed.
    pub truncated: bool,
}

/// Compute a line-level diff between `old` and `new`, grouped into hunks
/// with [`CONTEXT_LINES`] of context, capped at `line_cap` emitted lines.
///
/// `added`/`removed` always reflect the FULL diff, even when hunks are
/// truncated, so the `+N −M` summary stays accurate.
#[must_use]
pub fn compute_line_diff(old: &str, new: &str, line_cap: usize) -> FileDiff {
    let diff = TextDiff::from_lines(old, new);

    let mut added = 0usize;
    let mut removed = 0usize;
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut emitted = 0usize;
    let mut truncated = false;

    // `similar`'s grouped ops already coalesce changes with the requested
    // context radius into unified-diff-style groups.
    for group in diff.grouped_ops(CONTEXT_LINES).iter() {
        let mut lines: Vec<DiffLine> = Vec::new();
        let mut old_start = 0usize;
        let mut new_start = 0usize;
        let mut saw_anchor = false;

        for op in group {
            for change in diff.iter_changes(op) {
                if !saw_anchor {
                    // 1-based line numbers of the hunk's first line.
                    old_start = change.old_index().map_or(0, |i| i + 1);
                    new_start = change.new_index().map_or(0, |i| i + 1);
                    saw_anchor = true;
                }
                let kind = match change.tag() {
                    ChangeTag::Insert => {
                        added += 1;
                        DiffLineKind::Add
                    }
                    ChangeTag::Delete => {
                        removed += 1;
                        DiffLineKind::Del
                    }
                    ChangeTag::Equal => DiffLineKind::Ctx,
                };
                if emitted < line_cap {
                    let text = change.value().strip_suffix('\n').unwrap_or(change.value());
                    lines.push(DiffLine {
                        kind,
                        text: text.to_string(),
                    });
                    emitted += 1;
                } else {
                    truncated = true;
                }
            }
        }

        if !lines.is_empty() {
            hunks.push(DiffHunk {
                old_start,
                new_start,
                lines,
            });
        }
    }

    FileDiff {
        added,
        removed,
        hunks,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_content_yields_empty_diff() {
        let d = compute_line_diff("a\nb\nc\n", "a\nb\nc\n", DEFAULT_LINE_CAP);
        assert_eq!(d.added, 0);
        assert_eq!(d.removed, 0);
        assert!(d.hunks.is_empty());
        assert!(!d.truncated);
    }

    #[test]
    fn single_line_change_counts_add_and_del() {
        let d = compute_line_diff("a\nb\nc\n", "a\nB\nc\n", DEFAULT_LINE_CAP);
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 1);
        assert_eq!(d.hunks.len(), 1);
        // Context lines a and c bracket the change.
        let kinds: Vec<_> = d.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert!(kinds.contains(&DiffLineKind::Add));
        assert!(kinds.contains(&DiffLineKind::Del));
        assert!(kinds.contains(&DiffLineKind::Ctx));
    }

    #[test]
    fn pure_addition_has_no_removals() {
        let d = compute_line_diff("a\n", "a\nb\nc\n", DEFAULT_LINE_CAP);
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 0);
    }

    #[test]
    fn line_numbers_are_one_based() {
        // Change the first line; hunk anchors at line 1 on both sides.
        let d = compute_line_diff("a\nb\n", "A\nb\n", DEFAULT_LINE_CAP);
        assert_eq!(d.hunks[0].old_start, 1);
        assert_eq!(d.hunks[0].new_start, 1);
    }

    #[test]
    fn trailing_newline_stripped_from_line_text() {
        let d = compute_line_diff("a\n", "a\nb\n", DEFAULT_LINE_CAP);
        for hunk in &d.hunks {
            for line in &hunk.lines {
                assert!(!line.text.ends_with('\n'), "line text retained newline");
            }
        }
    }

    #[test]
    fn cap_truncates_and_counts_stay_full() {
        // 100 distinct old lines fully replaced by 100 distinct new lines.
        let old: String = (0..100).map(|i| format!("old{i}\n")).collect();
        let new: String = (0..100).map(|i| format!("new{i}\n")).collect();
        let d = compute_line_diff(&old, &new, 10);
        assert!(d.truncated, "expected truncation past the cap");
        let emitted: usize = d.hunks.iter().map(|h| h.lines.len()).sum();
        assert!(emitted <= 10, "emitted {emitted} lines, cap was 10");
        // Full counts survive truncation.
        assert_eq!(d.added, 100);
        assert_eq!(d.removed, 100);
    }
}
