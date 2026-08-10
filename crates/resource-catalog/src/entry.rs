//! The self-describing resource record.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::kind::{ResourceAddress, ResourceKind};

/// A self-describing resource record: a stable logical `name` mapped to a
/// typed physical [`ResourceAddress`] plus human-readable description and
/// metadata.
///
/// This is the unit of registration in the catalog and the unit of
/// serialization in the persistence layer. Because the record carries its own
/// description and metadata, the catalog is self-describing: an agent or
/// operator can enumerate every known resource and its meaning without any
/// external documentation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceEntry {
    /// Stable logical name, e.g. `"ldscore.1000g_eur"`. This is what callers
    /// resolve through the catalog; it never changes even if the physical
    /// address does.
    pub name: String,
    /// The resource kind, used for typed resolution and indexing.
    pub kind: ResourceKind,
    /// Human-readable, self-describing summary.
    pub description: String,
    /// The physical address, typed per-kind.
    pub address: ResourceAddress,
    /// Optional key-value metadata (e.g. population, panel version, format).
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Optional free-form tags for filtering/grouping.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl ResourceEntry {
    /// Construct an entry with empty metadata/tags.
    pub fn new(
        name: impl Into<String>,
        kind: ResourceKind,
        description: impl Into<String>,
        address: ResourceAddress,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            description: description.into(),
            address,
            metadata: BTreeMap::new(),
            tags: Vec::new(),
        }
    }

    /// Set metadata on the entry.
    pub fn with_metadata(mut self, metadata: BTreeMap<String, String>) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set tags on the entry.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}
