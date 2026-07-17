//! Centralized config — loaded once at boot, immutable thereafter.
//!
//! Precedence: env > $LETI_CONFIG_HOME/config.toml > XDG > defaults.
//! Currently env + defaults are wired; TOML parsing is not yet implemented.
//! SIGHUP-based reload deferred — MVP requires restart.

use std::env;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::tools::ToolSchedulerConfig;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub data_dir: PathBuf,
    pub default_model: String,
    pub permission_ruleset_path: Option<PathBuf>,
    pub log_format: LogFormat,
    pub plugins: PluginsConfig,
    pub tool_scheduler: ToolSchedulerConfig,
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
        let legacy = legacy_env_names(env::vars());
        if !legacy.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "legacy environment variables are not supported; rename or unset: {}",
                legacy.join(", ")
            )));
        }

        let bind_addr = env::var("LETI_BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_string());

        let data_dir = env::var("LETI_DATA_DIR")
            .map(|s| expand_tilde(&s))
            .unwrap_or_else(|_| default_data_dir());

        let default_model = env::var("LETI_DEFAULT_MODEL")
            .unwrap_or_else(|_| "anthropic/claude-sonnet-4-6".to_string());

        // max_cost_per_session_usd removed. Per-session cost
        // cap is cloud-only via the quota plugin; local binary has no
        // cap. Warn if operator still has the env var set.
        if env::var("LETI_MAX_COST_USD").is_ok() {
            tracing::warn!(
                "LETI_MAX_COST_USD is no longer honored; cost cap is plugin-driven (see test-quota-stub for reference). \
                 Remove the env var to silence this warning."
            );
        }

        let log_format = match env::var("LETI_LOG_FORMAT").as_deref() {
            Ok("pretty") => LogFormat::Pretty,
            _ => LogFormat::Json,
        };

        let permission_ruleset_path = env::var("LETI_PERMISSION_RULESET_PATH")
            .ok()
            .map(PathBuf::from);

        let parse_limit = |name: &str, default: usize| -> Result<usize, ConfigError> {
            match env::var(name) {
                Ok(raw) => raw.parse::<usize>().ok().filter(|v| *v > 0).ok_or_else(|| {
                    ConfigError::Invalid(format!("{name} must be a positive integer"))
                }),
                Err(_) => Ok(default),
            }
        };
        let tool_scheduler = ToolSchedulerConfig {
            max_per_turn: parse_limit("LETI_TOOL_MAX_PER_TURN", 8)?,
            max_global: parse_limit("LETI_TOOL_MAX_GLOBAL", 64)?,
        }
        .validate()
        .map_err(ConfigError::Invalid)?;

        Ok(Self {
            bind_addr,
            data_dir,
            default_model,
            permission_ruleset_path,
            log_format,
            plugins: PluginsConfig::default(),
            tool_scheduler,
        })
    }
}

fn default_data_dir() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".leti")
    } else {
        PathBuf::from(".leti")
    }
}

fn legacy_env_names<I>(vars: I) -> Vec<String>
where
    I: IntoIterator<Item = (String, String)>,
{
    vars.into_iter()
        .filter_map(|(name, _)| name.starts_with("OPENLET_").then_some(name))
        .collect()
}

/// Expands a leading `~` against `$HOME` so `LETI_DATA_DIR=~/.leti`
/// resolves to the home directory instead of creating a literal `./~/` tree.
/// Bare `~` and `~/...` expand; `~user` and absolute/relative paths pass through.
fn expand_tilde(raw: &str) -> PathBuf {
    if (raw == "~" || raw.starts_with("~/"))
        && let Ok(home) = env::var("HOME")
    {
        let rest = raw.strip_prefix("~/").or_else(|| raw.strip_prefix('~'));
        return match rest.filter(|s| !s.is_empty()) {
            Some(tail) => PathBuf::from(home).join(tail),
            None => PathBuf::from(home),
        };
    }
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_resolves_home_prefix() {
        let home = env::var("HOME").expect("HOME set in test env");
        assert_eq!(expand_tilde("~/.leti"), PathBuf::from(&home).join(".leti"));
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
    }

    #[test]
    fn expand_tilde_leaves_absolute_and_relative_untouched() {
        assert_eq!(
            expand_tilde("/var/lib/leti"),
            PathBuf::from("/var/lib/leti")
        );
        assert_eq!(expand_tilde("./data"), PathBuf::from("./data"));
        // `~user` is not home-expansion syntax we support — passes through verbatim.
        assert_eq!(expand_tilde("~bob/data"), PathBuf::from("~bob/data"));
    }

    #[test]
    fn legacy_openlet_environment_is_detected() {
        let names = legacy_env_names([
            ("LETI_BIND".to_string(), "127.0.0.1:8787".to_string()),
            ("OPENLET_BIND".to_string(), "0.0.0.0:8787".to_string()),
            ("OPENAI_API_KEY".to_string(), "provider-key".to_string()),
        ]);
        assert_eq!(names, vec!["OPENLET_BIND"]);
    }
}
