//! `ask_user` builtin tool — interactive multiple-choice prompt.
//!
//! A typed interactive prompt with a short header, a question, and a small
//! set of labeled options. The model picks via integer index.
//!
//! Wire flow:
//! 1. Tool runs → checks `SessionCapabilities::user_questions` (synchronous
//!    error if `false`, e.g. headless-cloud sessions).
//! 2. Claims the per-session pending slot (cap 1) → returns
//!    `question_already_pending` if another question is already in flight.
//! 3. Registers a oneshot in [`QuestionRegistry`], emits
//!    [`AgentEvent::QuestionRequested`], and awaits the receiver under
//!    a timeout.
//! 4. Frontend POSTs to `/v1/sessions/:id/question/answer` with the
//!    `question_id` + selected option indices.
//!
//! Permission string: `ask_user` (no per-input parameterization — the
//! ruleset can deny the tool wholesale, but the prompt content itself
//! is user-facing UX, not a privilege).

use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::permission::PermissionRequest;

/// Default frontend reply timeout. 5 minutes — long enough for a human
/// to read + respond, short enough that the model isn't stuck forever
/// if the frontend disconnects.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Maximum header length. Headers render in tight UI chrome (sidebars,
/// status pills); cap at 12 chars so they don't blow the layout.
pub const MAX_HEADER_LEN: usize = 12;

/// Minimum/maximum option counts. Need at least one option for the user
/// to pick; cap at a small number so the prompt stays scannable.
pub const MIN_OPTIONS: usize = 1;
pub const MAX_OPTIONS: usize = 8;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AskOptionInput {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AskUserInput {
    /// Short header (≤12 chars) rendered alongside the prompt.
    pub header: String,
    /// The question text shown to the user.
    pub question: String,
    /// Selectable options. 1..=8.
    pub options: Vec<AskOptionInput>,
    /// When true, the user may pick zero or more options. When false
    /// (default), exactly one selection is required.
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AskUserOutput {
    /// UUID assigned to this prompt for traceability.
    pub question_id: String,
    /// Indices into `options` selected by the user.
    pub selected: Vec<usize>,
    /// Echoes back the labels at the selected indices for convenience.
    pub selected_labels: Vec<String>,
}

pub struct AskUserTool {
    timeout: Duration,
}

impl AskUserTool {
    /// Construct with the default 300s reply timeout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Construct with a custom timeout. Tests pass tiny values to drive
    /// the timeout codepath without hanging the suite.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl Default for AskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AskUserTool {
    type Input = AskUserInput;
    type Output = AskUserOutput;

    fn name(&self) -> &'static str {
        "ask_user"
    }

    fn description(&self) -> &'static str {
        "Ask the user a typed multiple-choice question. Headers must be ≤12 chars; \
         options are 1..=8. Returns the indices the user picked. Fails synchronously \
         in headless sessions (capabilities.user_questions=false)."
    }

    fn parallel_safe(&self) -> bool {
        false
    }

    fn permission(&self, _input: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: "ask_user".to_string(),
            reason: None,
            timeout: None,
        }
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        crate::tools::builtins::ask_user_runner::run(self.timeout, ctx, input).await
    }
}
