use schemars::schema::Schema;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// Manifest a plugin declares — used for ordering, capability discovery,
/// version gating, and hook-skip optimization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: Version,
    pub description: String,
    pub author: Option<String>,
    pub capabilities: Vec<Capability>,
    pub core_version_req: VersionReq,
    pub default_priority: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<Schema>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Capability {
    Tool,
    Agent,
    Provider,
    Hook(crate::hooks::HookKind),
    Permission,
    Telemetry,
    Storage,
}
