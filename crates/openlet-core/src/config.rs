//! Centralized config — loaded once at boot, immutable thereafter.
//!
//! Precedence: env > $OPENLET_CONFIG_HOME/config.toml > XDG > defaults.
//! Currently env + defaults are wired; TOML parsing is not yet implemented.
//! SIGHUP-based reload deferred — MVP requires restart.

use std::env;
use std::path::PathBuf;

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
    pub fn load() -> Result<Self, ConfigError> {
        let bind_addr = env::var("OPENLET_BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_string());

        let data_dir = env::var("OPENLET_DATA_DIR")
            .map(|s| expand_tilde(&s))
            .unwrap_or_else(|_| default_data_dir());

        let openrouter_api_key = env::var("OPENROUTER_API_KEY").ok().map(SecretString::from);

        let default_model = env::var("OPENLET_DEFAULT_MODEL")
            .unwrap_or_else(|_| "anthropic/claude-sonnet-4-6".to_string());

        // max_cost_per_session_usd removed. Per-session cost
        // cap is cloud-only via the quota plugin; local binary has no
        // cap. Warn if operator still has the env var set.
        if env::var("OPENLET_MAX_COST_USD").is_ok() {
            tracing::warn!(
                "OPENLET_MAX_COST_USD is no longer honored; cost cap is plugin-driven (see test-quota-stub for reference). \
                 Remove the env var to silence this warning."
            );
        }

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

/// Expands a leading `~` against `$HOME` so `OPENLET_DATA_DIR=~/.openlet`
/// resolves to the home directory instead of creating a literal `./~/` tree.
/// Bare `~` and `~/...` expand; `~user` and absolute/relative paths pass through.
fn expand_tilde(raw: &str) -> PathBuf {
    if raw == "~" || raw.starts_with("~/") {
        if let Ok(home) = env::var("HOME") {
            let rest = raw.strip_prefix("~/").or_else(|| raw.strip_prefix('~'));
            return match rest.filter(|s| !s.is_empty()) {
                Some(tail) => PathBuf::from(home).join(tail),
                None => PathBuf::from(home),
            };
        }
    }
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_resolves_home_prefix() {
        let home = env::var("HOME").expect("HOME set in test env");
        assert_eq!(
            expand_tilde("~/.openlet"),
            PathBuf::from(&home).join(".openlet")
        );
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
    }

    #[test]
    fn expand_tilde_leaves_absolute_and_relative_untouched() {
        assert_eq!(
            expand_tilde("/var/lib/openlet"),
            PathBuf::from("/var/lib/openlet")
        );
        assert_eq!(expand_tilde("./data"), PathBuf::from("./data"));
        // `~user` is not home-expansion syntax we support — passes through verbatim.
        assert_eq!(expand_tilde("~bob/data"), PathBuf::from("~bob/data"));
    }
}
