use schemars::schema::Schema;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// Default `core_version_req` for plugins targeting the v0.1 core
/// surface. Centralised so plugins don't each hand-roll
/// `VersionReq::parse(">=0.1.0").expect("...")`.
#[must_use]
pub fn core_version_req_v0_1() -> VersionReq {
    VersionReq::parse(">=0.1.0").expect("static version req is parseable")
}

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
