//! Approved runtime environments used by plugin policy validation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A reusable runtime environment selected by a plugin.
///
/// This is not a plugin-owned image: ordinary plugin repositories contain
/// source and tests, while environment images are managed as shared assets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub reference: String,
    pub interpreters: Vec<String>,
}

/// Approved digest-pinned runtime environments available to plugins.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvironmentCatalog {
    environments: BTreeMap<String, Environment>,
}

impl EnvironmentCatalog {
    pub fn insert(&mut self, id: impl Into<String>, environment: Environment) {
        self.environments.insert(id.into(), environment);
    }

    pub fn remove(&mut self, id: &str) {
        self.environments.remove(id);
    }

    pub fn get(&self, id: &str) -> Option<&Environment> {
        self.environments.get(id)
    }

    /// Return a sorted snapshot of approved runtime environments.
    pub fn list(&self) -> BTreeMap<String, Environment> {
        self.environments.clone()
    }

    /// Return the catalog id owning one digest-pinned reference.
    pub fn find_reference(&self, reference: &str) -> Option<&str> {
        self.environments
            .iter()
            .find(|(_, environment)| environment.reference == reference)
            .map(|(id, _)| id.as_str())
    }
}
