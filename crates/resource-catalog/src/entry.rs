//! The self-describing resource record.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::archive::ArchiveStatus;
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
    /// Optional archive configuration: where to back up this resource's
    /// content in cloud object storage. When set, the catalog can
    /// [`archive`](crate::catalog::ResourceCatalog::archive) /
    /// [`restore`](crate::catalog::ResourceCatalog::restore) via rclone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_spec: Option<crate::archive::ArchiveSpec>,
    /// Runtime archive state: tracks the last archive/restore/verify result.
    /// Updated by archive operations; persisted with the manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_status: Option<ArchiveStatus>,
    /// Optional ingestion spec: how to load source files into this storage
    /// backend. When set, an ingestion executor can ingest data from the
    /// declared source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingestion_spec: Option<crate::ingestion::IngestionSpec>,
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
            archive_spec: None,
            archive_status: None,
            ingestion_spec: None,
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

    /// Set the archive spec on the entry, declaring where this resource's
    /// content is backed up in cloud object storage.
    pub fn with_archive(mut self, spec: crate::archive::ArchiveSpec) -> Self {
        self.archive_spec = Some(spec);
        self
    }

    /// Set the ingestion spec on the entry, declaring
    /// how to load source files into this table.
    pub fn with_ingestion(mut self, spec: crate::ingestion::IngestionSpec) -> Self {
        self.ingestion_spec = Some(spec);
        self
    }
}
