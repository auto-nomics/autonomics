//! The resource registry: a name-keyed store with a kind index.

use std::collections::HashMap;

use crate::entry::ResourceEntry;
use crate::error::{ResourceError, Result};
use crate::kind::ResourceKind;
use crate::validate::validate;

/// The in-memory resource index, keyed by stable logical name, with a
/// secondary index by [`ResourceKind`].
///
/// Mirrors the shape of `NodeRegistry` / `ToolRegistry` in the workspace:
/// a `HashMap` plus `register`/`get`/`list` accessors. Registration validates
/// the entry and rejects duplicate names with a conflicting address.
#[derive(Debug, Default)]
pub struct ResourceRegistry {
    by_name: HashMap<String, ResourceEntry>,
    by_kind: HashMap<ResourceKind, Vec<String>>,
}

impl ResourceRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a resource. Validates the entry first; a duplicate name is
    /// a no-op if the address is identical (idempotent re-registration from a
    /// reloaded manifest or a re-run provider), and an error if the address
    /// differs.
    pub fn register(&mut self, entry: ResourceEntry) -> Result<()> {
        validate(&entry)?;
        if let Some(existing) = self.by_name.get(&entry.name) {
            if existing.address == entry.address && existing.kind == entry.kind {
                // Idempotent: same logical name, same physical address.
                return Ok(());
            }
            return Err(ResourceError::Duplicate(entry.name));
        }
        self.by_kind
            .entry(entry.kind)
            .or_default()
            .push(entry.name.clone());
        self.by_name.insert(entry.name.clone(), entry);
        Ok(())
    }

    /// Look up a resource by logical name.
    pub fn get(&self, name: &str) -> Option<&ResourceEntry> {
        self.by_name.get(name)
    }

    /// Mutable lookup — needed for in-place archive_status updates.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut ResourceEntry> {
        self.by_name.get_mut(name)
    }

    /// Remove an entry by name. Returns the removed entry if it existed.
    pub fn remove(&mut self, name: &str) -> Option<ResourceEntry> {
        let entry = self.by_name.remove(name)?;
        // Also remove from by_kind index.
        if let Some(names) = self.by_kind.get_mut(&entry.kind) {
            names.retain(|n| n != name);
        }
        Some(entry)
    }

    /// All registered resources.
    pub fn list(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.by_name.values()
    }

    /// All resources of a given kind.
    pub fn list_by_kind(&self, kind: ResourceKind) -> impl Iterator<Item = &ResourceEntry> {
        self.by_kind
            .get(&kind)
            .into_iter()
            .flatten()
            .filter_map(move |name| self.by_name.get(name))
    }

    /// Number of registered resources.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Collect all entries into a `Vec` (for serialization).
    pub fn into_entries(&self) -> Vec<ResourceEntry> {
        self.by_name.values().cloned().collect()
    }
}
