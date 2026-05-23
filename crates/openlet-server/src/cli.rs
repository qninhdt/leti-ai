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
    /// Path to the session log to audit. Phase 8 implements.
    pub session_log: Option<std::path::PathBuf>,
}

impl Cli {
    /// Resolves the effective subcommand, falling back to `Serve` with
    /// default args if none was given.
    pub fn resolved_command(self) -> Command {
        self.command
            .unwrap_or(Command::Serve(ServeArgs { bind: None }))
    }
}
