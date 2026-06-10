//! Outbound `CredentialProvider` seam.
//!
//! When an agent tool calls out to an openlet service, it carries
//! openlet-ai's own service-account credential scoped to the agent's
//! workspace — NOT a per-turn user-delegated token. openlet-ai ships the
//! trait + a local no-op default; the cloud impl (openlet repo) returns
//! the real SA bearer.
//!
//! NOTE: no outbound HTTP tool exists in core today (every builtin is
//! local: bash/edit/read/write/glob/grep/list/todo/ask_user/plan_mode/
//! subagent_task/task_status). This seam is built and held in `AppState`
//! but NOT yet threaded into tool calls — when a real outbound tool
//! lands it reads the credential from the tool context. Wiring dead
//! plumbing into a non-existent sink now would be untestable.

use async_trait::async_trait;
use secrecy::SecretString;

use super::principal::AgentWorkspace;

/// Why an outbound credential could not be minted. Surfaced to the
/// calling tool, never to an end client.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// The provider could not issue a credential for this workspace
    /// (control-plane unreachable, workspace unknown to the issuer).
    #[error("credential issuance failed: {0}")]
    IssuanceFailed(String),
}

/// An outbound bearer credential scoped to one agent workspace. The
/// secret is held in `SecretString` so it never lands in a `Debug` dump
/// or a log line by accident.
#[derive(Clone)]
pub struct OutboundCredential {
    /// Bearer token value (sent as `Authorization: Bearer <token>`).
    pub bearer: SecretString,
}

impl std::fmt::Debug for OutboundCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret. Confirm presence only.
        f.debug_struct("OutboundCredential")
            .field("bearer", &"<redacted>")
            .finish()
    }
}

/// Outbound credential seam. Returns the SA credential an agent's
/// outbound calls should carry, scoped to the workspace being acted upon.
#[async_trait]
pub trait CredentialProvider: Send + Sync + 'static {
    /// Mint (or look up) the outbound credential for `workspace`.
    /// `Ok(None)` means "no credential" — the local posture, where agent
    /// tools make no authenticated outbound calls.
    async fn workspace_credential(
        &self,
        workspace: &AgentWorkspace,
    ) -> Result<Option<OutboundCredential>, CredentialError>;
}

/// Local-binary default: no outbound credential. Agent tools run against
/// the local workspace and call no authenticated openlet services.
#[derive(Debug, Clone, Default)]
pub struct NoopCredentialProvider;

#[async_trait]
impl CredentialProvider for NoopCredentialProvider {
    async fn workspace_credential(
        &self,
        _workspace: &AgentWorkspace,
    ) -> Result<Option<OutboundCredential>, CredentialError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlet_core::types::agent::AgentId;

    fn ws() -> AgentWorkspace {
        AgentWorkspace {
            agent_id: AgentId::new(),
            workspace_root: std::path::PathBuf::from("/tmp/ws"),
            owner_principal_id: "local-dev".into(),
        }
    }

    #[tokio::test]
    async fn noop_returns_no_credential() {
        let p = NoopCredentialProvider;
        assert!(p.workspace_credential(&ws()).await.unwrap().is_none());
    }
}
