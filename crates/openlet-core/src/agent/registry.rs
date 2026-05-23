//! `AgentRegistry` — slug → `AgentDefinition`. Built once at boot from
//! plugin `install` calls; immutable thereafter.

use std::collections::HashMap;

use thiserror::Error;

use super::definition::AgentDefinition;
use super::slug::{AgentSlug, SlugError};

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("agent slug already registered: {0}")]
    Duplicate(AgentSlug),
    #[error(transparent)]
    Slug(#[from] SlugError),
}

#[derive(Default, Debug)]
pub struct AgentRegistry {
    by_slug: HashMap<AgentSlug, AgentDefinition>,
}

impl AgentRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, def: AgentDefinition) -> Result<(), RegistryError> {
        let slug = def.slug.clone();
        if self.by_slug.contains_key(&slug) {
            return Err(RegistryError::Duplicate(slug));
        }
        self.by_slug.insert(slug, def);
        Ok(())
    }

    #[must_use]
    pub fn get(&self, slug: &AgentSlug) -> Option<&AgentDefinition> {
        self.by_slug.get(slug)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&AgentSlug, &AgentDefinition)> {
        self.by_slug.iter()
    }

    pub fn iter_visible(&self) -> impl Iterator<Item = (&AgentSlug, &AgentDefinition)> {
        self.by_slug.iter().filter(|(_, d)| !d.hidden)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_slug.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_slug.is_empty()
    }
}
