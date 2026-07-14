//! `send_message` tool — deliver a message to a live sibling subagent.
//!
//! Phase 4 (security-hardened). A named live subagent can message another
//! live sibling addressed by its unique roster handle name. The delivery:
//!   1. resolves `to` in the session-scoped roster, restricted to
//!      SAME-PARENT siblings by default (hierarchy containment — a message
//!      can't cross branches unless the sender's agent def opts in);
//!   2. runs a NAME-SAFETY generation check — if the handle now points to
//!      a different task than the caller's snapshot, the send is refused
//!      (no silent misroute to a recycled name);
//!   3. runs a PRIVILEGE check — the sender's tool allowlist must cover
//!      the receiver's, so a low-privilege agent can't escalate by asking
//!      a high-privilege peer to act (confused-deputy containment);
//!   4. pushes the (length-bounded) body onto the receiver's inbox, which
//!      the receiver's re-armable driver drains as an untrusted
//!      `SiblingMessage`-origin turn (never authoritative instructions).
//!
//! The message BODY never rides the `subagent.message` SSE frame (that is
//! activity metadata only) — it is delivered in-band as untrusted data.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::runtime::subagent::{HandleName, TaskRegistry};
use crate::tools::Tool;
use crate::types::permission::PermissionRequest;
use crate::types::session::SessionId;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    /// Unique roster handle name of the live sibling to message (e.g.
    /// `reviewer` or `reviewer#2`). Get live names from the subagent
    /// roster; a name that no longer resolves returns a typed error.
    pub to: String,
    /// Plain-text message body. Delivered to the recipient as UNTRUSTED
    /// data (not instructions). Length-bounded by the registry.
    pub body: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SendMessageOutput {
    /// `true` when the message was accepted onto the recipient's inbox.
    pub delivered: bool,
    /// The resolved recipient handle name (echoed for the model's log).
    pub to: String,
}

pub struct SendMessageTool {
    registry: Arc<TaskRegistry>,
}

impl SendMessageTool {
    #[must_use]
    pub fn new(registry: Arc<TaskRegistry>) -> Self {
        Self { registry }
    }
}

/// `true` if `sender_allowlist` covers `receiver_allowlist` — i.e. every
/// tool the receiver may use, the sender may also use. An EMPTY allowlist
/// means inherit-all (maximally privileged), so an empty sender covers any
/// receiver, and a non-empty sender can NOT cover an (inherit-all) empty
/// receiver. This blocks a low-privilege sender from escalating by
/// messaging a higher-privilege peer (security Finding 1).
fn allowlist_covers(sender: &[String], receiver: &[String]) -> bool {
    if sender.is_empty() {
        return true; // inherit-all sender covers everything
    }
    if receiver.is_empty() {
        return false; // receiver is inherit-all; a scoped sender can't cover it
    }
    receiver.iter().all(|t| sender.contains(t))
}

#[async_trait]
impl Tool for SendMessageTool {
    type Input = SendMessageInput;
    type Output = SendMessageOutput;

    fn name(&self) -> &'static str {
        "send_message"
    }
    fn description(&self) -> &'static str {
        "Send a message to a live sibling subagent by its unique handle name. The recipient \
         receives it as untrusted data on its next turn. Only same-parent siblings are reachable; \
         you cannot message a sibling that requires more tool permissions than you hold."
    }
    fn parallel_safe(&self) -> bool {
        true
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple(format!("send_message:{}", input.to))
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        // Resolve the sender's session → parent + root, and its own tool
        // allowlist (from its current agent def) for the privilege check.
        let sender_meta = ctx
            .memory
            .get_session(ctx.session_id)
            .await
            .map_err(|e| ToolError::Io(format!("send_message: sender session lookup: {e}")))?
            .ok_or_else(|| {
                ToolError::InvalidInput("send_message: sender session missing".into())
            })?;
        let sender_parent = sender_meta.parent_session_id.unwrap_or(ctx.session_id);
        let root = root_of(&ctx, ctx.session_id).await?;

        // Resolve the sender's own agent def to get its tool allowlist. This
        // MUST fail CLOSED: if the sender's identity can't be resolved (no
        // `current_agent_slug`, or the slug isn't a registered agent), we do
        // NOT fall back to an empty allowlist — an empty allowlist means
        // "inherit-all / maximally privileged" in `allowlist_covers`, so an
        // unresolved sender would be treated as able to message ANY peer,
        // reopening the exact confused-deputy escalation Finding 1 blocks.
        // Instead we refuse the send outright (security Finding 1, fail-closed).
        let sender_allowlist: Vec<String> = sender_meta
            .current_agent_slug
            .as_deref()
            .and_then(|s| crate::agent::AgentSlug::new(s.to_string()).ok())
            .and_then(|slug| ctx.agent_registry.get(&slug))
            .map(|def| def.tool_allowlist.clone())
            .ok_or_else(|| {
                ToolError::InvalidInput(
                    "send_message: refused — sender agent identity unresolved (fail-closed; \
                     cannot verify privilege)"
                        .into(),
                )
            })?;

        let to_name = HandleName(input.to.clone());
        let entry = self.registry.resolve_name(root, &to_name).ok_or_else(|| {
            ToolError::InvalidInput(format!(
                "send_message: target '{}' not addressable (unknown or finalized)",
                input.to
            ))
        })?;

        // Hierarchy scope (security Finding 4): default reachability is
        // SAME-PARENT siblings only. A cross-branch / ancestor↔descendant
        // send is refused unless the sender opts in (deferred: MVP is
        // same-parent only; opt-in reads the sender agent def flag).
        if entry.parent != sender_parent {
            return Err(ToolError::InvalidInput(format!(
                "send_message: target '{}' is not a same-parent sibling (cross-branch messaging \
                 not permitted)",
                input.to
            )));
        }

        // Privilege check (security Finding 1): sender must cover receiver.
        if !allowlist_covers(&sender_allowlist, &entry.allowlist) {
            return Err(ToolError::InvalidInput(format!(
                "send_message: refused — messaging '{}' would escalate privilege (recipient holds \
                 tools the sender does not)",
                input.to
            )));
        }

        // Push onto the recipient's inbox (length + depth bounded). A
        // typed error (unknown/over-length/full) surfaces to the model.
        // The `from` provenance is the sender's session id; the recipient's
        // untrusted-data framing renders it as `from=...` so a malicious
        // body can't spoof a trusted sender identity.
        let from = format!("session:{}", ctx.session_id);
        self.registry
            .push_message(entry.task_id, &from, &input.body)
            .map_err(|e| ToolError::InvalidInput(format!("{}: {}", e.code(), e)))?;

        // Emit the `subagent.message` activity frame (metadata only — the body
        // rides the recipient's untrusted turn, never this frame) so the TUI
        // social panel can surface cross-sibling activity. Best-effort.
        use crate::adapters::event_sink::Persistence;
        use crate::types::event::AgentEvent;
        let _ = ctx
            .events
            .publish(
                AgentEvent::SubagentMessage {
                    task_id: entry.task_id.0,
                    parent_session_id: entry.parent,
                    from,
                    to: input.to.clone(),
                },
                Persistence::Durable,
            )
            .await;

        Ok(SendMessageOutput {
            delivered: true,
            to: input.to,
        })
    }
}

/// Resolve the root session by walking `parent_session_id`. Bounded walk
/// (depth cap + 2) mirrors the spawner's `root_session_of`; a store error
/// fails the send rather than guessing the wrong root (mis-scoped roster).
async fn root_of(ctx: &ToolCtx, start: SessionId) -> Result<SessionId, ToolError> {
    let mut current = start;
    for _ in 0..(crate::runtime::subagent::DEFAULT_MAX_DEPTH as usize + 2) {
        match ctx.memory.get_session(current).await {
            Ok(Some(meta)) => match meta.parent_session_id {
                Some(p) => current = p,
                None => return Ok(current),
            },
            Ok(None) => return Ok(current),
            Err(e) => return Err(ToolError::Io(format!("send_message: root resolution: {e}"))),
        }
    }
    Ok(current)
}
