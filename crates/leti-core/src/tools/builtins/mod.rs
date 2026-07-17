//! Built-in tools (`read`, `list`, `glob`, `grep`, `write`, `edit`,
//! `bash`, `todo`, and the `Option`-injected `web_fetch`) plus the
//! interactive `ask_user` prompt tool implementing the typed `Tool` trait.
//! Each tool is a single small file under this module.
//!
//! Production wiring registers these through the `core-tools` plugin
//! (`crates/leti-plugins/core-tools/src/lib.rs`), which is the
//! canonical custom-tool example for downstream integrators.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Shared captured output of a subprocess-style tool (`bash` / `python`).
/// `BashOutput` and `PythonOutput` are field-identical aliases of this —
/// the two executors return the same shape, only their permission string
/// and default timeout differ.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProcessOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

pub mod ask_user;
pub(crate) mod ask_user_runner;
pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod list;
pub mod plan_mode;
pub mod python;
pub mod read;
pub mod send_message;
pub mod subagent_control;
pub mod subagent_task;
pub mod task_status;
pub mod todo;
pub mod web_fetch;
pub mod write;

pub use ask_user::AskUserTool;
pub use bash::BashTool;
pub use edit::EditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use list::ListTool;
pub use plan_mode::{EnterPlanModeTool, ExitPlanModeTool};
pub use python::{PythonExecutor, PythonOutput, PythonTool};
pub use read::ReadTool;
pub use send_message::SendMessageTool;
pub use subagent_control::{
    SubagentCancelTool, SubagentContinueTool, SubagentInterruptTool, SubagentListTool,
};
pub use subagent_task::{SubagentSpawner, SubagentTaskTool};
pub use task_status::TaskStatusTool;
pub use todo::TodoTool;
pub use web_fetch::{FetchError, FetchFormat, FetchRequest, FetchedPage, WebFetchTool, WebFetcher};
pub use write::WriteTool;
