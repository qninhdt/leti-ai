//! `openlet-server audit` subcommand — pretty-print a session JSONL log
//! with a defense-in-depth redaction pass.
//!
//! Reading the file again here (instead of trusting the writer) catches
//! cases where a new event shape sneaked a token past the writer's
//! allowlist. The redactor is the same one the writer uses, applied a
//! second time to the parsed envelope.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use openlet_adapters::localfs::SecretRedactor;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use uuid::Uuid;

use crate::cli::{AuditArgs, AuditFormat};

pub async fn run(args: AuditArgs, data_dir: &Path) -> Result<()> {
    let path = resolve_path(&args, data_dir)?;
    let from = parse_ts(args.from.as_deref(), "--from")?;
    let to = parse_ts(args.to.as_deref(), "--to")?;
    let redactor = SecretRedactor::default();

    let file = tokio::fs::File::open(&path)
        .await
        .with_context(|| format!("opening session log {}", path.display()))?;
    let mut reader = BufReader::new(file).lines();

    let mut line_no: usize = 0;
    while let Some(line) = reader
        .next_line()
        .await
        .context("reading session log line")?
    {
        line_no += 1;
        if line.is_empty() {
            continue;
        }
        let mut envelope: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("warn: line {line_no}: skipping invalid json ({e})");
                continue;
            }
        };

        let ts = envelope
            .get("ts")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc));
        if let Some(t) = ts {
            if let Some(lo) = from
                && t < lo
            {
                continue;
            }
            if let Some(hi) = to
                && t > hi
            {
                continue;
            }
        }

        // Defense-in-depth redaction.
        redactor.redact_in_place(&mut envelope);

        match args.format {
            AuditFormat::Json => {
                let s = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".into());
                println!("{s}");
            }
            AuditFormat::Pretty => print_pretty(&envelope, line_no),
        }
    }

    Ok(())
}

fn resolve_path(args: &AuditArgs, data_dir: &Path) -> Result<PathBuf> {
    if let Some(file) = &args.file {
        // `--file` is a convenience for replaying rotated logs that the
        // SessionLogger has renamed (`<id>.jsonl.1`). Constrain it to
        // the data_dir/sessions/ subtree so the audit pretty-printer
        // can't be repurposed as a "redact-and-print arbitrary file"
        // tool. Symlink escapes are handled by canonicalizing both
        // sides and asserting prefix.
        let sessions_root = data_dir.join("sessions");
        let canonical_root = std::fs::canonicalize(&sessions_root)
            .with_context(|| format!("canonicalizing sessions root {}", sessions_root.display()))?;
        let canonical_file = std::fs::canonicalize(file)
            .with_context(|| format!("canonicalizing --file {}", file.display()))?;
        if !canonical_file.starts_with(&canonical_root) {
            return Err(anyhow!(
                "--file must resolve under {} (got {})",
                canonical_root.display(),
                canonical_file.display()
            ));
        }
        return Ok(canonical_file);
    }
    let id = args
        .session_id
        .as_ref()
        .ok_or_else(|| anyhow!("audit: pass --session-id <ID> or --file <PATH>"))?;
    // Reject non-UUID session ids before path-building so a caller
    // can't escape the sessions/ directory via `..` / absolute paths /
    // path separators baked into the input.
    let parsed =
        Uuid::parse_str(id).with_context(|| format!("--session-id must be a UUID, got `{id}`"))?;
    Ok(data_dir.join("sessions").join(format!("{parsed}.jsonl")))
}

fn parse_ts(s: Option<&str>, flag: &str) -> Result<Option<DateTime<Utc>>> {
    let Some(v) = s else { return Ok(None) };
    let parsed = DateTime::parse_from_rfc3339(v)
        .with_context(|| format!("parsing {flag} as RFC3339: {v}"))?;
    Ok(Some(parsed.with_timezone(&Utc)))
}

fn print_pretty(envelope: &Value, line_no: usize) {
    let ts = envelope
        .get("ts")
        .and_then(Value::as_str)
        .unwrap_or("(no-ts)");
    let event = envelope.get("event").unwrap_or(&Value::Null);
    let kind = event
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            event
                .as_object()
                .and_then(|o| o.keys().next().map(String::as_str))
                .unwrap_or("?")
        });
    let summary = serde_json::to_string(event).unwrap_or_else(|_| "{}".into());
    let snippet = if summary.len() > 240 {
        // `floor_char_boundary` (stable on 1.79+) walks back to the
        // nearest UTF-8 boundary so we never slice through a multibyte
        // codepoint when an event payload contains non-ASCII.
        let cutoff = floor_char_boundary(&summary, 240);
        format!("{}…", &summary[..cutoff])
    } else {
        summary
    };
    println!("{ts} [{kind}] (line {line_no}) {snippet}");
}

/// Inline copy of the unstable `str::floor_char_boundary` so audit
/// printing never panics on multibyte cuts.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlet_adapters::localfs::SecretRedactor;

    #[test]
    fn redactor_scrubs_planted_secrets_in_string_and_object_fields() {
        let r = SecretRedactor::default();
        let mut v = serde_json::json!({
            "ts": "2026-05-23T10:00:00Z",
            "event": {
                "api_key": "sk-this-is-secret-1234567890",
                "Authorization": "Bearer abc.def.ghi",
                "text": "leak sk-abcdef0123456789xyz inline"
            }
        });
        r.redact_in_place(&mut v);
        let dumped = serde_json::to_string(&v).unwrap();
        assert!(
            !dumped.contains("sk-this-is-secret"),
            "api_key not redacted: {dumped}"
        );
        assert!(
            !dumped.contains("Bearer abc"),
            "auth not redacted: {dumped}"
        );
        assert!(
            !dumped.contains("sk-abcdef0123456789xyz"),
            "inline sk- not redacted: {dumped}"
        );
    }

    #[test]
    fn parse_ts_round_trips_rfc3339() {
        let parsed = parse_ts(Some("2026-05-23T10:00:05Z"), "--from").unwrap();
        assert!(parsed.is_some());
        let bad = parse_ts(Some("not a date"), "--from");
        assert!(bad.is_err());
    }
}
