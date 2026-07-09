//! Emulated Python — an in-process [Monty](https://github.com/pydantic/monty)
//! VM driven through the [`Filesystem`](openlet_core::adapters::filesystem::Filesystem)
//! seam.
//!
//! Replaces a `python3` subprocess with a deny-by-default interpreter designed
//! for untrusted LLM code: `os.system`, `socket`, and `subprocess` are simply
//! not importable, memory / duration are bounded by Monty's own resource
//! limits, and every filesystem hop routes through the injected `Filesystem`
//! rather than the host disk. Local vs cloud differ only in which
//! `Filesystem` impl is injected — the interpreter is identical.
//!
//! Module layout:
//! - `mount_bridge` — maps each Monty `OsFunctionCall` variant onto `ctx.fs`
//! - `executor`     — the `PythonExecutor` trait impl the runtime injects

mod executor;
mod mount_bridge;

pub use executor::{default_max_memory, run_python, MontyExecutor};
