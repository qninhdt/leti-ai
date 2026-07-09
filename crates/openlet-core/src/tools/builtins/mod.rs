//! Eight built-in tools (`read`, `list`, `glob`, `grep`, `write`, `edit`,
//! `bash`, `todo`) plus the interactive `ask_user` prompt tool implementing
//! the typed `Tool` trait. Each tool is a single small file under this
//! module.
//!
//! Production wiring registers these through the `core-tools` plugin
//! (`crates/openlet-plugins/core-tools/src/lib.rs`), which is the
//! canonical custom-tool example for downstream integrators.

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
pub mod subagent_task;
pub mod task_status;
pub mod todo;
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
pub use subagent_task::{SubagentSpawner, SubagentTaskTool};
pub use task_status::TaskStatusTool;
pub use todo::TodoTool;
pub use write::WriteTool;
