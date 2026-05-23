//! Agent slug — kebab-case identifier used to route sessions to definitions.
//!
//! Distinct from `AgentId` (UUIDv4 principal). An `AgentSlug` is the
//! human-typed name in `POST /v1/session { agent: "general" }`; the slug
//! is resolved to an `AgentDefinition` via `AgentRegistry::get`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentSlug(String);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SlugError {
    #[error("agent slug is empty")]
    Empty,
    #[error("agent slug must be kebab-case (a-z 0-9 -) and 2..=64 chars: {0}")]
    Invalid(String),
}

impl AgentSlug {
    pub fn new(s: impl Into<String>) -> Result<Self, SlugError> {
        let s = s.into();
        if s.is_empty() {
            return Err(SlugError::Empty);
        }
        if !(2..=64).contains(&s.len()) {
            return Err(SlugError::Invalid(s));
        }
        let ok = s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !s.starts_with('-')
            && !s.ends_with('-')
            && !s.contains("--");
        if !ok {
            return Err(SlugError::Invalid(s));
        }
        Ok(Self(s))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AgentSlug {
    type Err = SlugError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_kebab_case() {
        assert!(AgentSlug::new("general").is_ok());
        assert!(AgentSlug::new("indexer-stub").is_ok());
        assert!(AgentSlug::new("a1-b2").is_ok());
    }

    #[test]
    fn rejects_uppercase_underscore_edges() {
        assert!(AgentSlug::new("General").is_err());
        assert!(AgentSlug::new("snake_case").is_err());
        assert!(AgentSlug::new("-leading").is_err());
        assert!(AgentSlug::new("trailing-").is_err());
        assert!(AgentSlug::new("double--dash").is_err());
        assert!(AgentSlug::new("a").is_err());
        assert!(AgentSlug::new("").is_err());
    }
}
