//! CLI surface — `serve` (default) and `audit` (Phase 8 placeholder).
//!
//! Per amendment §O, clap derive structure is locked early so phase-08
//! does not need a restructure.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "openlet-server",
    version,
    about = "Openlet agent runtime",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the HTTP + SSE server (default if no subcommand is given).
    Serve(ServeArgs),
    /// Audit subcommand — Phase 8 implements; reserved here.
    Audit(AuditArgs),
    /// Preflight diagnostics — checks API key, data dir, sqlite, plugins,
    /// model reachability, and bind port. Read-only.
    Doctor(DoctorArgs),
}

#[derive(Debug, Parser)]
pub struct ServeArgs {
    /// Override the bind address resolved by `Config` (env or default).
    /// `Config` already reads `OPENLET_BIND`; this flag wins over both.
    #[arg(long)]
    pub bind: Option<String>,
}

#[derive(Debug, Parser)]
pub struct AuditArgs {
    /// Session id whose `<data_dir>/sessions/<id>.jsonl` should be printed.
    /// Mutually exclusive with `--file`.
    #[arg(long, value_name = "SESSION_ID")]
    pub session_id: Option<String>,

    /// Explicit path to a JSONL session log. Overrides `--session-id`.
    #[arg(long, value_name = "PATH")]
    pub file: Option<std::path::PathBuf>,

    /// RFC3339 lower bound on the envelope `ts` field.
    #[arg(long)]
    pub from: Option<String>,

    /// RFC3339 upper bound on the envelope `ts` field.
    #[arg(long)]
    pub to: Option<String>,

    /// Output format. `pretty` is human-readable; `json` re-emits the
    /// (re-redacted) envelope verbatim for piping into jq / log shippers.
    #[arg(long, value_enum, default_value_t = AuditFormat::Pretty)]
    pub format: AuditFormat,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum AuditFormat {
    Pretty,
    Json,
}

#[derive(Debug, Parser)]
pub struct DoctorArgs {
    /// Output format. `text` is human-readable with status glyphs;
    /// `json` emits the redacted DoctorReport for piping to jq.
    #[arg(long, value_enum, default_value_t = DoctorFormat::Text)]
    pub format: DoctorFormat,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum DoctorFormat {
    Text,
    Json,
}

impl Cli {
    /// Resolves the effective subcommand, falling back to `Serve` with
    /// default args if none was given.
    pub fn resolved_command(self) -> Command {
        self.command
            .unwrap_or(Command::Serve(ServeArgs { bind: None }))
    }
}
