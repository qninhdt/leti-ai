//! `edit` tool — find/replace with read-before-write + unique-match gate.
//!
//! Single-replace mode requires the `find` text to appear exactly once
//! (Anthropic str_replace semantics; tighter than a
//! `first_index == last_index` shortcut). `replace_all` switches to
//! verbatim `String::replace`. We reject ambiguous matches (rather than
//! replacing the first) because they silently corrupt files.
//!
//! A single call may carry a BATCH of ops (`edits`): they are applied
//! sequentially against an in-memory buffer, then the result is written
//! ONCE (all-or-nothing). Any op that fails aborts the whole call with no
//! write, so the file on disk is never left half-edited. Each non-
//! `replace_all` op re-checks the strict uniqueness gate against the
//! CURRENT buffer state, so ordering is well-defined: a later op sees the
//! text produced by the earlier ops. No fuzzy matching is added — `edit`
//! stays exact-match and surgical.

use std::path::PathBuf;

use crate::adapters::filesystem::WriteOpts;
use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::permission::{PermissionMode, PermissionRequest};

/// A single find/replace operation. `replace_all=false` (default) requires
/// `find` to be unique in the current buffer; `true` replaces every match.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EditOp {
    pub find: String,
    pub replace: String,
    #[serde(default)]
    pub replace_all: bool,
}

/// The `edits` field: either a single op object or a list of ops. Untagged
/// so both `{"find":..,"replace":..}` and `[{..},{..}]` deserialize.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EditsInput {
    /// A list of ops, applied in order.
    Many(Vec<EditOp>),
    /// A single op.
    One(EditOp),
}

/// Tool input. Untagged so THREE shapes deserialize:
/// - batch: `{ "path": .., "edits": {..} }` or `{ "path": .., "edits": [..] }`
/// - legacy flat: `{ "path": .., "find": .., "replace": .., "replace_all"?: bool }`
///
/// `Batch` is tried first; it requires an `edits` field, so the flat shape
/// (which has no `edits`) falls through to `Flat`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EditInput {
    /// Batch form — one or more ops under `edits`.
    Batch { path: PathBuf, edits: EditsInput },
    /// Legacy single-op flat form. Kept for backward compatibility.
    Flat {
        path: PathBuf,
        find: String,
        replace: String,
        #[serde(default)]
        replace_all: bool,
    },
}

impl EditInput {
    /// The target path, common to both shapes.
    fn path(&self) -> &PathBuf {
        match self {
            EditInput::Batch { path, .. } | EditInput::Flat { path, .. } => path,
        }
    }

    /// Normalize either shape into `(path, ops)`.
    fn into_parts(self) -> (PathBuf, Vec<EditOp>) {
        match self {
            EditInput::Batch { path, edits } => {
                let ops = match edits {
                    EditsInput::Many(ops) => ops,
                    EditsInput::One(op) => vec![op],
                };
                (path, ops)
            }
            EditInput::Flat {
                path,
                find,
                replace,
                replace_all,
            } => (
                path,
                vec![EditOp {
                    find,
                    replace,
                    replace_all,
                }],
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EditOutput {
    pub path: String,
    /// Total replacements across all ops in the batch.
    pub replacements: usize,
    /// Line-level diff of the whole change (original vs final buffer), for
    /// TUI rendering. Rides through the tool-result JSON body — no protocol
    /// change (see `tools::diff`).
    pub diff: crate::tools::diff::FileDiff,
}

/// Apply one op against `buffer`, returning the new buffer and the number of
/// replacements made. Enforces the same gates as the single-edit path:
/// non-empty `find`, `find != replace` (no-op rejection), presence, and the
/// strict uniqueness gate for non-`replace_all` ops. Kept as a free function
/// so the uniqueness logic is unit-testable without a filesystem.
fn apply_edit_op(buffer: &str, op: &EditOp) -> Result<(String, usize), ToolError> {
    if op.find.is_empty() {
        return Err(ToolError::InvalidInput(
            "find string must not be empty".into(),
        ));
    }
    if op.find == op.replace {
        return Err(ToolError::InvalidInput(
            "find and replace are identical — no-op".into(),
        ));
    }
    let occurrences = buffer.matches(&op.find).count();
    if occurrences == 0 {
        return Err(ToolError::InvalidInput(format!(
            "find string not present: {:?}",
            op.find
        )));
    }
    if op.replace_all {
        Ok((buffer.replace(&op.find, &op.replace), occurrences))
    } else {
        if occurrences > 1 {
            return Err(ToolError::InvalidInput(format!(
                "find string matches {occurrences} locations — pass replace_all=true or add more context"
            )));
        }
        Ok((buffer.replacen(&op.find, &op.replace, 1), 1))
    }
}

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    type Input = EditInput;
    type Output = EditOutput;

    fn name(&self) -> &'static str {
        "edit"
    }
    fn description(&self) -> &'static str {
        "Find/replace inside a file. Single-match by default; pass replace_all=true for global. \
         Pass a batch via `edits` (a single op or a list) to apply several ops in one atomic \
         write — ops run in order against the running buffer, and any failure aborts the whole \
         call with no write. Read first."
    }
    fn parallel_safe(&self) -> bool {
        false
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple(format!("edit:{}", input.path().display()))
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let (path, ops) = input.into_parts();

        if ops.is_empty() {
            return Err(ToolError::InvalidInput(
                "edits must contain at least one op".into(),
            ));
        }

        if !ctx.mode.permits(PermissionMode::Danger) && !ctx.read_history.contains(&path).await {
            return Err(ToolError::ReadBeforeWriteRequired(
                path.display().to_string(),
            ));
        }

        let bytes = ctx.fs.read(&path, None).await?;
        let original = String::from_utf8(bytes.to_vec())
            .map_err(|_| ToolError::InvalidInput("file is not valid UTF-8".into()))?;

        // Apply every op in-memory. Any error aborts before we touch disk.
        let mut buffer = original.clone();
        let mut total_replacements = 0usize;
        for op in &ops {
            let (next, made) = apply_edit_op(&buffer, op)?;
            buffer = next;
            total_replacements += made;
        }

        let diff = crate::tools::diff::compute_line_diff(
            &original,
            &buffer,
            crate::tools::diff::DEFAULT_LINE_CAP,
        );

        let body = Bytes::from(buffer.into_bytes());
        let _meta = ctx.fs.write(&path, body, WriteOpts::default()).await?;

        Ok(EditOutput {
            path: path.display().to_string(),
            replacements: total_replacements,
            diff,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_op_replaces_once() {
        let (out, n) = apply_edit_op(
            "hello world",
            &EditOp {
                find: "world".into(),
                replace: "rust".into(),
                replace_all: false,
            },
        )
        .unwrap();
        assert_eq!(out, "hello rust");
        assert_eq!(n, 1);
    }

    #[test]
    fn ambiguous_single_op_errors() {
        let res = apply_edit_op(
            "foo foo foo",
            &EditOp {
                find: "foo".into(),
                replace: "bar".into(),
                replace_all: false,
            },
        );
        assert!(matches!(res, Err(ToolError::InvalidInput(_))));
    }

    #[test]
    fn replace_all_counts_every_occurrence() {
        let (out, n) = apply_edit_op(
            "foo foo foo",
            &EditOp {
                find: "foo".into(),
                replace: "bar".into(),
                replace_all: true,
            },
        )
        .unwrap();
        assert_eq!(out, "bar bar bar");
        assert_eq!(n, 3);
    }

    #[test]
    fn zero_match_errors() {
        let res = apply_edit_op(
            "hello",
            &EditOp {
                find: "absent".into(),
                replace: "x".into(),
                replace_all: false,
            },
        );
        assert!(matches!(res, Err(ToolError::InvalidInput(_))));
    }

    #[test]
    fn identical_find_replace_is_noop_error() {
        let res = apply_edit_op(
            "hello",
            &EditOp {
                find: "hello".into(),
                replace: "hello".into(),
                replace_all: false,
            },
        );
        assert!(matches!(res, Err(ToolError::InvalidInput(_))));
    }

    #[test]
    fn empty_find_errors() {
        let res = apply_edit_op(
            "hello",
            &EditOp {
                find: String::new(),
                replace: "x".into(),
                replace_all: false,
            },
        );
        assert!(matches!(res, Err(ToolError::InvalidInput(_))));
    }

    #[test]
    fn ops_apply_sequentially_against_running_buffer() {
        // op1 turns "a" into "b"; op2 then finds "b" (produced by op1).
        let (out1, _) = apply_edit_op(
            "a",
            &EditOp {
                find: "a".into(),
                replace: "b".into(),
                replace_all: false,
            },
        )
        .unwrap();
        let (out2, _) = apply_edit_op(
            &out1,
            &EditOp {
                find: "b".into(),
                replace: "c".into(),
                replace_all: false,
            },
        )
        .unwrap();
        assert_eq!(out2, "c");
    }

    #[test]
    fn legacy_flat_shape_deserializes() {
        let json = serde_json::json!({
            "path": "a.md",
            "find": "world",
            "replace": "rust"
        });
        let input: EditInput = serde_json::from_value(json).unwrap();
        let (path, ops) = input.into_parts();
        assert_eq!(path, PathBuf::from("a.md"));
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].find, "world");
        assert!(!ops[0].replace_all);
    }

    #[test]
    fn batch_single_object_deserializes() {
        let json = serde_json::json!({
            "path": "a.md",
            "edits": { "find": "x", "replace": "y", "replace_all": true }
        });
        let input: EditInput = serde_json::from_value(json).unwrap();
        let (_, ops) = input.into_parts();
        assert_eq!(ops.len(), 1);
        assert!(ops[0].replace_all);
    }

    #[test]
    fn batch_list_deserializes_in_order() {
        let json = serde_json::json!({
            "path": "a.md",
            "edits": [
                { "find": "a", "replace": "b" },
                { "find": "c", "replace": "d" }
            ]
        });
        let input: EditInput = serde_json::from_value(json).unwrap();
        let (_, ops) = input.into_parts();
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].find, "a");
        assert_eq!(ops[1].find, "c");
    }
}
