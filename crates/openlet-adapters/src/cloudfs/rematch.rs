//! Phase-2 of cloud grep: in-process regex re-match over prefilter candidates.
//!
//! The backend (`GrepFiles`) returns candidate rows whose `extracted_text`
//! passed a literal trigram prefilter. The caller-controlled regex NEVER runs
//! in Postgres; it runs HERE, over the fetched text, with the same
//! `regex::RegexBuilder` the local `LocalFilesystem::grep` uses. That is the
//! dialect-parity guarantee: a pattern accepted locally is accepted in cloud
//! and vice-versa (both are RE2/linear-time — no backtracking, so a
//! catastrophic pattern is rejected identically on both sides, and neither can
//! hang).

use openlet_core::adapters::filesystem::{GrepArgs, GrepHit};
use openlet_core::error::FsError;
use regex::{Regex, RegexBuilder};
use std::path::PathBuf;

/// A candidate row to re-match: workspace-relative path + its indexed body.
pub(crate) struct Candidate {
    pub path: PathBuf,
    pub text: String,
}

/// Compile the caller pattern with the SAME options as the local grep engine.
/// Fails with `FsError::InvalidInput` on an unparseable pattern — identical to
/// the local path, so a bad regex is a bad regex on both backends.
pub(crate) fn compile(args: &GrepArgs) -> Result<Regex, FsError> {
    RegexBuilder::new(&args.pattern)
        .case_insensitive(args.case_insensitive)
        .build()
        .map_err(|e| FsError::InvalidInput(e.to_string()))
}

/// Floor `index` to the nearest UTF-8 char boundary at or below it. Mirrors the
/// local engine so truncated hit text is byte-identical across backends.
fn floor_char_boundary(s: &str, mut index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Re-match `candidates` against the compiled pattern, emitting `GrepHit`s in
/// the same shape (`line` 1-indexed, `text` truncated at `max_line_chars` with
/// a trailing `...`) as `LocalFilesystem::grep`. Bounded by `args.max_hits`.
///
/// `re` is pre-compiled by [`compile`] so a caller can reject a bad pattern
/// before doing any network work.
pub(crate) fn rematch(re: &Regex, candidates: &[Candidate], args: &GrepArgs) -> Vec<GrepHit> {
    let mut hits: Vec<GrepHit> = Vec::new();
    'outer: for cand in candidates {
        for (idx, line) in cand.text.lines().enumerate() {
            if hits.len() >= args.max_hits {
                break 'outer;
            }
            if re.is_match(line) {
                let text = if line.len() > args.max_line_chars {
                    let cut = floor_char_boundary(line, args.max_line_chars);
                    format!("{}...", &line[..cut])
                } else {
                    line.to_string()
                };
                hits.push(GrepHit {
                    path: cand.path.clone(),
                    line: (idx + 1) as u64,
                    text,
                });
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pattern: &str) -> GrepArgs {
        GrepArgs {
            pattern: pattern.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn rematch_finds_regex_hit_across_lines() {
        let a = args("TODO.*urgent");
        let re = compile(&a).unwrap();
        let cands = vec![Candidate {
            path: PathBuf::from("notes.txt"),
            text: "line one\nTODO fix this urgent thing\nline three".to_string(),
        }];
        let hits = rematch(&re, &cands, &a);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 2);
        assert_eq!(hits[0].path, PathBuf::from("notes.txt"));
    }

    #[test]
    fn rematch_respects_max_hits() {
        let mut a = args("x");
        a.max_hits = 2;
        let re = compile(&a).unwrap();
        let cands = vec![Candidate {
            path: PathBuf::from("f"),
            text: "x\nx\nx\nx".to_string(),
        }];
        assert_eq!(rematch(&re, &cands, &a).len(), 2);
    }

    #[test]
    fn rematch_truncates_long_line() {
        let mut a = args("start");
        a.max_line_chars = 10;
        let re = compile(&a).unwrap();
        let long = format!("start{}", "z".repeat(100));
        let cands = vec![Candidate {
            path: PathBuf::from("f"),
            text: long,
        }];
        let hits = rematch(&re, &cands, &a);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.ends_with("..."));
        assert_eq!(hits[0].text.len(), 10 + 3);
    }

    #[test]
    fn case_insensitive_matches_like_local() {
        let mut a = args("todo");
        a.case_insensitive = true;
        let re = compile(&a).unwrap();
        let cands = vec![Candidate {
            path: PathBuf::from("f"),
            text: "This is a TODO".to_string(),
        }];
        assert_eq!(rematch(&re, &cands, &a).len(), 1);
    }

    #[test]
    fn backreference_pattern_rejected_same_as_local() {
        // The Rust `regex` crate rejects backreferences (no backtracking).
        // Local grep rejects it too via the same builder — dialect parity.
        let a = args(r"(\w+)\s+\1");
        assert!(compile(&a).is_err());
    }
}
