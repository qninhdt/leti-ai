//! Eight built-in tools (`read`, `list`, `glob`, `grep`, `write`, `edit`,
//! `bash`, `todo`) implementing the typed `Tool` trait. Each tool is a
//! single small file under this module.

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod list;
pub mod read;
pub mod todo;
pub mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use list::ListTool;
pub use read::ReadTool;
pub use todo::TodoTool;
pub use write::WriteTool;

use std::sync::Arc;

use crate::tools::ToolRegistry;

use self::bash::ShellExecutor;

/// Build the standard registry with the eight built-in tools.
/// `shell` is the bash executor — pass `LocalShellExecutor` from the
/// adapter crate, or any custom impl (e.g. mock for tests).
#[must_use]
pub fn default_registry(shell: Arc<dyn ShellExecutor>) -> Arc<ToolRegistry> {
    ToolRegistry::builder()
        .register(ReadTool)
        .register(ListTool)
        .register(GlobTool)
        .register(GrepTool)
        .register(WriteTool)
        .register(EditTool)
        .register(BashTool::with_executor(shell))
        .register(TodoTool)
        .build()
}
