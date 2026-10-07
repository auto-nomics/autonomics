//! Host-owned runtime environment allow list.

//! The catalog is mutable only through the daemon. Agents read a snapshot,
//! while user-approved changes update one shared registry and persist
//! atomically.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use container_runtime::ImageReference;

use crate::{
    Environment, EnvironmentCatalog, Error, Result, names::validate_plugin_name,
    request::atomic_toml,
};

/// Persistent allow list shared by validation and agent tools.
#[derive(Clone)]
pub struct EnvironmentRegistry {
    catalog: Arc<RwLock<EnvironmentCatalog>>,
    path: Option<PathBuf>,
}

impl EnvironmentRegistry {
    /// Create an in-memory registry used by tests and embedded callers.
    pub fn ephemeral(catalog: EnvironmentCatalog) -> Self {
        Self {
            catalog: Arc::new(RwLock::new(catalog)),
            path: None,
        }
    }

    /// Load the persistent environment file, seeding it on first launch.
    ///
    /// Once the file exists it is authoritative, so removing a seeded
    /// environment is not undone by the next daemon start.
    pub fn open(path: impl Into<PathBuf>, seed: EnvironmentCatalog) -> Result<Self> {
        let path = path.into();
        let catalog = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|source| Error::ParseToml {
                path: path.clone(),
                source,
            })?,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => seed,
            Err(source) => {
                return Err(Error::ReadFile {
                    path: path.clone(),
                    source,
                });
            }
        };
        validate_catalog(&catalog)?;
        if !path.exists() {
            atomic_toml(&path, &catalog)?;
        }
        Ok(Self {
            catalog: Arc::new(RwLock::new(catalog)),
            path: Some(path),
        })
    }

    /// Return a consistent snapshot of the approved environments.
    pub fn snapshot(&self) -> EnvironmentCatalog {
        self.lock(|catalog| Ok(catalog.clone()))
            .expect("environment registry lock poisoned")
    }

    /// Return one environment by id.
    pub fn get(&self, id: &str) -> Result<Option<Environment>> {
        self.lock(|catalog| Ok(catalog.get(id).cloned()))
    }

    /// Return the id owning one digest-pinned image reference.
    pub fn find_reference(&self, reference: &str) -> Result<Option<String>> {
        self.lock(|catalog| Ok(catalog.find_reference(reference).map(str::to_string)))
    }

    /// Insert or replace an environment after explicit user approval.
    pub fn approve(&self, id: &str, environment: Environment) -> Result<Environment> {
        validate_environment(id, &environment)?;
        self.lock(|catalog| {
            catalog.insert(id, environment.clone());
            if let Some(path) = &self.path {
                atomic_toml(path, catalog)?;
            }
            Ok(environment)
        })
    }

    /// Remove an environment from future plugin development.
    pub fn remove(&self, id: &str) -> Result<bool> {
        self.lock(|catalog| {
            if catalog.get(id).is_none() {
                return Ok(false);
            }
            catalog.remove(id);
            if let Some(path) = &self.path {
                atomic_toml(path, catalog)?;
            }
            Ok(true)
        })
    }

    fn lock<T>(&self, operation: impl FnOnce(&mut EnvironmentCatalog) -> Result<T>) -> Result<T> {
        let mut catalog = self
            .catalog
            .write()
            .map_err(|_| Error::Validation("environment registry lock poisoned".into()))?;
        operation(&mut catalog)
    }
}

fn validate_catalog(catalog: &EnvironmentCatalog) -> Result<()> {
    for (id, environment) in catalog.list() {
        validate_environment(&id, &environment)?;
    }
    Ok(())
}

fn validate_environment(id: &str, environment: &Environment) -> Result<()> {
    if let Err(error) = validate_plugin_name(id) {
        return Err(Error::InvalidRequest(format!(
            "environment id `{id}` is invalid: {error}"
        )));
    }
    ImageReference::parse(&environment.reference)
        .map_err(|error| Error::Validation(format!("environment `{id}`: {error}")))?;
    if environment.interpreters.is_empty() {
        return Err(Error::Validation(format!(
            "environment `{id}` must declare at least one interpreter"
        )));
    }
    let mut normalized = BTreeMap::new();
    for interpreter in &environment.interpreters {
        let value = interpreter.trim().to_ascii_lowercase();
        if value.is_empty() {
            return Err(Error::Validation(format!(
                "environment `{id}` contains an empty interpreter"
            )));
        }
        normalized.insert(value, ());
    }
    if normalized.len() != environment.interpreters.len() {
        return Err(Error::Validation(format!(
            "environment `{id}` contains duplicate interpreters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALPINE_REFERENCE: &str = "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123";
    const PYTHON_REFERENCE: &str = "docker.io/library/python@sha256:1234567890123456789012345678901234567890123456789012345678901234";

    fn seed() -> EnvironmentCatalog {
        let mut catalog = EnvironmentCatalog::default();
        catalog.insert(
            "alpine",
            Environment {
                reference: ALPINE_REFERENCE.into(),
                interpreters: vec!["sh".into()],
            },
        );
        catalog
    }

    #[test]
    fn user_approval_persists_and_replaces_the_allow_list() {
        let state = tempfile::tempdir().unwrap();
        let path = state.path().join("plugin-environments.toml");
        let registry = EnvironmentRegistry::open(&path, seed()).unwrap();
        assert!(path.is_file());

        registry
            .approve(
                "python",
                Environment {
                    reference: PYTHON_REFERENCE.into(),
                    interpreters: vec!["python3".into()],
                },
            )
            .unwrap();
        assert!(registry.get("python").unwrap().is_some());

        assert!(registry.remove("alpine").unwrap());
        let reloaded = EnvironmentRegistry::open(&path, seed()).unwrap();
        assert!(reloaded.get("alpine").unwrap().is_none());
        assert_eq!(
            reloaded.get("python").unwrap().unwrap().reference,
            PYTHON_REFERENCE
        );
    }

    #[test]
    fn approval_rejects_mutable_and_malformed_references() {
        let registry = EnvironmentRegistry::ephemeral(seed());
        let invalid = [
            "docker.io/library/alpine:latest",
            "docker.io/library/alpine@sha256:short",
        ];
        for reference in invalid {
            assert!(
                registry
                    .approve(
                        "candidate",
                        Environment {
                            reference: reference.into(),
                            interpreters: vec!["sh".into()],
                        },
                    )
                    .is_err(),
                "reference `{reference}` should be rejected"
            );
        }
    }
}
