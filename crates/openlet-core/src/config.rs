//! Centralized config — loaded once at boot, immutable thereafter.
//!
//! Per amendment §J: env > $OPENLET_CONFIG_HOME/config.toml > XDG > defaults.
//! Phase 1 implements env + defaults; TOML parsing lands in Phase 8 polish.
//! SIGHUP-based reload deferred — MVP requires restart.

use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use rust_decimal::Decimal;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub data_dir: PathBuf,
    pub openrouter_api_key: Option<SecretString>,
    pub default_model: String,
    pub permission_ruleset_path: Option<PathBuf>,
    pub max_cost_per_session_usd: Decimal,
    pub log_format: LogFormat,
    pub plugins: PluginsConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Json,
    Pretty,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginsConfig {
    /// Whitelist of plugin ids to enable (empty = all compiled-in).
    #[serde(default)]
    pub enabled: Vec<String>,
    /// Explicit deny list — wins over `enabled`.
    #[serde(default)]
    pub disabled: Vec<String>,
}

impl Config {
    /// Loads config with precedence: env > defaults.
    /// TOML file support lands in Phase 8.
    pub fn load() -> Result<Self, ConfigError> {
        let bind_addr = env::var("OPENLET_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8787".to_string());

        let data_dir = env::var("OPENLET_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_data_dir());

        let openrouter_api_key = env::var("OPENROUTER_API_KEY")
            .ok()
            .map(SecretString::from);

        let default_model = env::var("OPENLET_DEFAULT_MODEL")
            .unwrap_or_else(|_| "anthropic/claude-sonnet-4-6".to_string());

        let max_cost = env::var("OPENLET_MAX_COST_USD")
            .ok()
            .and_then(|s| Decimal::from_str(&s).ok())
            .unwrap_or_else(|| Decimal::new(5, 0));

        let log_format = match env::var("OPENLET_LOG_FORMAT").as_deref() {
            Ok("pretty") => LogFormat::Pretty,
            _ => LogFormat::Json,
        };

        let permission_ruleset_path = env::var("OPENLET_PERMISSION_RULESET_PATH")
            .ok()
            .map(PathBuf::from);

        Ok(Self {
            bind_addr,
            data_dir,
            openrouter_api_key,
            default_model,
            permission_ruleset_path,
            max_cost_per_session_usd: max_cost,
            log_format,
            plugins: PluginsConfig::default(),
        })
    }
}

fn default_data_dir() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".openlet")
    } else {
        PathBuf::from(".openlet")
    }
}
