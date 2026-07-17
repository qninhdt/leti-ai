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

impl PluginManifest {
    /// Start building a manifest with the two required fields.
    #[must_use]
    pub fn builder(id: impl Into<String>, name: impl Into<String>) -> PluginManifestBuilder {
        PluginManifestBuilder {
            id: id.into(),
            name: name.into(),
            version: Version::new(0, 1, 0),
            description: String::new(),
            author: None,
            capabilities: Vec::new(),
            core_version_req: core_version_req_v0_1(),
            default_priority: 100,
            config_schema: None,
        }
    }
}

/// Builder for [`PluginManifest`] with sensible defaults.
///
/// Required: `id`, `name` (set via [`PluginManifest::builder`]).
/// Defaults: `core_version_req` = `>=0.1.0`, `default_priority` = 100,
/// `capabilities` = `[]`, `config_schema` = `None`, `version` = `0.1.0`.
#[derive(Debug, Clone)]
pub struct PluginManifestBuilder {
    id: String,
    name: String,
    version: Version,
    description: String,
    author: Option<String>,
    capabilities: Vec<Capability>,
    core_version_req: VersionReq,
    default_priority: u8,
    config_schema: Option<Schema>,
}

impl PluginManifestBuilder {
    #[must_use]
    pub fn version(mut self, v: Version) -> Self {
        self.version = v;
        self
    }

    #[must_use]
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    #[must_use]
    pub fn author(mut self, a: impl Into<String>) -> Self {
        self.author = Some(a.into());
        self
    }

    #[must_use]
    pub fn capabilities(mut self, c: Vec<Capability>) -> Self {
        self.capabilities = c;
        self
    }

    #[must_use]
    pub fn core_version_req(mut self, r: VersionReq) -> Self {
        self.core_version_req = r;
        self
    }

    #[must_use]
    pub fn default_priority(mut self, p: u8) -> Self {
        self.default_priority = p;
        self
    }

    #[must_use]
    pub fn config_schema(mut self, s: Schema) -> Self {
        self.config_schema = Some(s);
        self
    }

    /// Consume the builder and produce the manifest.
    #[must_use]
    pub fn build(self) -> PluginManifest {
        PluginManifest {
            id: self.id,
            name: self.name,
            version: self.version,
            description: self.description,
            author: self.author,
            capabilities: self.capabilities,
            core_version_req: self.core_version_req,
            default_priority: self.default_priority,
            config_schema: self.config_schema,
        }
    }
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
