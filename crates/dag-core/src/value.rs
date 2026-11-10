//! Typed values carried by DAG edges.

use datafusion::prelude::DataFrame;
use serde::{Deserialize, Serialize};

use crate::dag::DagError;

/// The runtime payload carried by one edge.
#[derive(Debug, Clone)]
pub enum NodeValue {
    DataFrame(DataFrame),
    File(FileRef),
    FileSet(Vec<FileRef>),
    Channel(ChannelValue),
    /// One channel delivery containing nested typed data and artifacts.
    Structured(crate::dag::execution::ChannelTransportValue),
}

impl NodeValue {
    pub fn data_type(&self) -> PortType {
        match self {
            Self::DataFrame(_) => PortType::DataFrame,
            Self::File(_) => PortType::File,
            Self::FileSet(_) => PortType::FileSet,
            Self::Channel(_) => PortType::Channel,
            Self::Structured(_) => PortType::Channel,
        }
    }

    pub fn as_dataframe(&self) -> Result<&DataFrame, DagError> {
        match self {
            Self::DataFrame(df) => Ok(df),
            other => Err(DagError::Schedule(format!(
                "expected a DataFrame value, got {}",
                other.data_type()
            ))),
        }
    }

    pub fn as_file(&self) -> Result<&FileRef, DagError> {
        match self {
            Self::File(file) => Ok(file),
            other => Err(DagError::Schedule(format!(
                "expected a File value, got {}",
                other.data_type()
            ))),
        }
    }

    pub fn as_file_set(&self) -> Result<&[FileRef], DagError> {
        match self {
            Self::FileSet(files) => Ok(files),
            other => Err(DagError::Schedule(format!(
                "expected a FileSet value, got {}",
                other.data_type()
            ))),
        }
    }

    pub fn as_channel(&self) -> Result<&ChannelValue, DagError> {
        match self {
            Self::Channel(channel) => Ok(channel),
            other => Err(DagError::Schedule(format!(
                "expected a Channel value, got {}",
                other.data_type()
            ))),
        }
    }

    pub fn as_structured_channel(
        &self,
    ) -> Result<&crate::dag::execution::ChannelTransportValue, DagError> {
        match self {
            Self::Structured(value) => Ok(value),
            other => Err(DagError::Schedule(format!(
                "expected a structured Channel value, got {}",
                other.data_type()
            ))),
        }
    }
}

impl From<DataFrame> for NodeValue {
    fn from(df: DataFrame) -> Self {
        Self::DataFrame(df)
    }
}

impl From<FileRef> for NodeValue {
    fn from(file: FileRef) -> Self {
        Self::File(file)
    }
}

impl From<Vec<FileRef>> for NodeValue {
    fn from(files: Vec<FileRef>) -> Self {
        Self::FileSet(files)
    }
}

/// A deterministic, JSON-native dataflow channel.
///
/// Channels are streams of serializable items. They are distinct from graph
/// edges: an edge routes one runtime value, while a channel value carries the
/// items used to instantiate downstream jobs or compose new channels.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelValue {
    pub items: Vec<serde_json::Value>,
}

impl ChannelValue {
    /// Move items through a bounded asynchronous queue.
    ///
    /// The current scheduler still waits for a node to finish before its
    /// downstream graph node starts; this method gives every channel operator
    /// the bounded asynchronous transfer discipline that incremental edge
    /// readiness will use once scheduler streaming is enabled.
    pub async fn via_bounded_stream(self, capacity: usize) -> std::result::Result<Self, DagError> {
        let capacity = capacity.max(1);
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<serde_json::Value>(capacity);
        let producer = tokio::spawn(async move {
            for item in self.items {
                if sender.send(item).await.is_err() {
                    break;
                }
            }
        });

        let mut items = Vec::new();
        while let Some(item) = receiver.recv().await {
            items.push(item);
        }
        producer
            .await
            .map_err(|error| DagError::Schedule(format!("channel producer failed: {error}")))?;
        Ok(Self { items })
    }
}

impl From<ChannelValue> for NodeValue {
    fn from(channel: ChannelValue) -> Self {
        Self::Channel(channel)
    }
}

/// The filesystem domain a file placement addresses.
///
/// `local` (the default, and the historical meaning of every record
/// persisted before domains existed) is the coordinator's own
/// filesystem; any other name refers to a [`crate::value`] placement
/// inside a catalog-declared [`dag_core`] data domain. The id is an
/// opaque string: the compute catalog owns its vocabulary
/// (see `runtime::compute::DataDomain`), and consumers treat an
/// unknown id conservatively — as *not* their own domain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DomainId(String);

impl DomainId {
    pub const LOCAL_NAME: &'static str = "local";

    pub fn local() -> Self {
        Self(Self::LOCAL_NAME.to_string())
    }

    /// Name a placement domain. The catalog validates ids at enrollment;
    /// here only emptiness is rejected so hand-built ids fail loudly.
    pub fn new(name: &str) -> Self {
        Self(name.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_local(&self) -> bool {
        self.0 == Self::LOCAL_NAME
    }
}

impl Default for DomainId {
    fn default() -> Self {
        Self::local()
    }
}

/// A file artifact address passed between nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "FileRefWire")]
pub struct FileRef {
    /// Absolute or engine-mounted path. Relative paths are not accepted at
    /// runtime because they depend on the caller's process working directory.
    pub path: String,
    /// Optional normalized format label (for example `csv`, `vcf`, `parquet`).
    pub format: Option<String>,
    pub fingerprint: Option<FileFingerprint>,
    /// The filesystem domain this placement addresses. Defaults to
    /// `local` for every record persisted before domains existed, and
    /// is omitted from serialization entirely when local, so the wire
    /// format of local files is byte-identical to the pre-domain form.
    #[serde(default, skip_serializing_if = "DomainId::is_local")]
    pub domain: DomainId,
}

// Keep legacy records readable while making a domain URI authoritative.
#[derive(Deserialize)]
struct FileRefWire {
    path: String,
    format: Option<String>,
    fingerprint: Option<FileFingerprint>,
    domain: Option<DomainId>,
}

impl TryFrom<FileRefWire> for FileRef {
    type Error = String;

    fn try_from(wire: FileRefWire) -> Result<Self, Self::Error> {
        let addressed = vfs::uri::domain(&wire.path).map(DomainId::new);
        if let (Some(addressed), Some(declared)) = (&addressed, &wire.domain) {
            if addressed != declared {
                return Err(format!(
                    "file URI domain '{}' disagrees with declared domain '{}'",
                    addressed.as_str(),
                    declared.as_str()
                ));
            }
        }
        Ok(Self {
            path: wire.path,
            format: wire.format,
            fingerprint: wire.fingerprint,
            domain: addressed.or(wire.domain).unwrap_or_default(),
        })
    }
}

/// A location-independent contract for an object stored by the DAG data plane.
///
/// `uri` is normally a `vfs://` address. The fingerprint is content-based so
/// it remains valid after an object is copied between nodes or object stores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub uri: String,
    pub format: Option<String>,
    pub fingerprint: Option<FileFingerprint>,
    pub producer: Option<String>,
    pub generation: Option<u64>,
}

impl From<ArtifactRef> for FileRef {
    fn from(artifact: ArtifactRef) -> Self {
        let domain = vfs::uri::domain(&artifact.uri)
            .map(DomainId::new)
            .unwrap_or_default();
        Self {
            path: artifact.uri,
            format: artifact.format,
            fingerprint: artifact.fingerprint,
            domain,
        }
    }
}

impl FileRef {
    pub fn new(path: impl Into<String>, format: Option<String>) -> Self {
        let path = path.into();
        let domain = vfs::uri::domain(&path)
            .map(DomainId::new)
            .unwrap_or_default();
        Self {
            path,
            format,
            fingerprint: None,
            domain,
        }
    }

    /// Upgrade a mounted legacy reference using the current mount table.
    /// Host paths retain their identity; no filesystem bytes are moved.
    pub fn canonicalized(mut self, storage: &vfs::OpendalFileStorage) -> Result<Self, DagError> {
        if vfs::uri::storage_path(&self.path).is_none() {
            return Ok(self);
        }
        let path = storage
            .canonical_uri(&self.path)
            .map_err(|e| DagError::Schedule(e.to_string()))?;
        let domain = DomainId::new(vfs::uri::domain(&path).unwrap_or("local"));
        if !self.domain.is_local() && self.domain != domain {
            return Err(DagError::Schedule(
                "file domain disagrees with its mounted address".into(),
            ));
        }
        self.path = path;
        self.domain = domain;
        Ok(self)
    }

    /// Rebind this placement to an explicit filesystem domain. Use at the
    /// publication/staging points that know which catalog-declared domain
    /// the content physically lives in.
    pub fn in_domain(mut self, domain: DomainId) -> Self {
        self.domain = domain;
        self
    }

    pub fn local(
        path: impl AsRef<std::path::Path>,
        format: Option<String>,
    ) -> Result<Self, DagError> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path).map_err(|e| {
            DagError::Schedule(format!("cannot stat file `{}`: {e}", path.display()))
        })?;
        if !metadata.is_file() {
            return Err(DagError::Schedule(format!(
                "file output `{}` is not a regular file",
                path.display()
            )));
        }
        Ok(Self {
            path: path.to_string_lossy().into_owned(),
            format,
            fingerprint: Some(FileFingerprint::from_metadata(&metadata)),
            domain: DomainId::local(),
        })
    }

    /// Build a remote artifact address. Remote fingerprints must not depend on
    /// worker-local mtimes.
    pub fn remote(
        uri: impl Into<String>,
        format: Option<String>,
        size: u64,
        sha256: Option<String>,
    ) -> Self {
        let path = uri.into();
        let domain = vfs::uri::domain(&path)
            .map(DomainId::new)
            .unwrap_or_default();
        Self {
            path,
            format,
            fingerprint: Some(FileFingerprint::remote(size, sha256)),
            domain,
        }
    }
}

/// Cheap artifact identity suitable for incremental invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub size: u64,
    pub mtime_ns: i128,
    pub content_hash: Option<String>,
    /// Declared at the value's **birth** (publish/staging point): the content
    /// lives on the immutable object store, so a recorded content hash makes
    /// the file permanently clean for incremental invalidation — regardless
    /// of how the path is spelled (with or without the `vfs://` prefix).
    ///
    /// Deliberately excluded from equality reasoning by consumers: freshness
    /// checks compare fields explicitly rather than through [`PartialEq`].
    #[serde(default)]
    pub immutable_remote: bool,
}

impl FileFingerprint {
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            size: metadata.len(),
            mtime_ns: metadata
                .modified()
                .ok()
                .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos() as i128)
                .unwrap_or_default(),
            content_hash: None,
            immutable_remote: false,
        }
    }

    pub fn from_path(path: impl AsRef<std::path::Path>) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        metadata.is_file().then(|| Self::from_metadata(&metadata))
    }

    pub fn remote(size: u64, sha256: Option<String>) -> Self {
        Self {
            size,
            mtime_ns: 0,
            content_hash: sha256,
            immutable_remote: true,
        }
    }
}

/// The payload type declared by a node port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    DataFrame,
    File,
    FileSet,
    Channel,
    Any,
}

impl std::fmt::Display for PortType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DataFrame => "dataframe",
            Self::File => "file",
            Self::FileSet => "file_set",
            Self::Channel => "channel",
            Self::Any => "any",
        })
    }
}

impl PortType {
    pub fn accepts(&self, actual: PortType) -> bool {
        *self == PortType::Any || actual == PortType::Any || *self == actual
    }
}

#[cfg(test)]
mod domain_tests {
    use super::*;

    #[test]
    fn legacy_mounted_reference_upgrades_without_changing_content_identity() {
        let root = tempfile::tempdir().unwrap();
        let manifest = vfs::VfsManifest::from_toml(&format!(
            "[[backend]]\nid='shared'\ntype='local'\nroot={}\n[[mount]]\npath='/cluster'\nbackend='shared'\nsource='/'\ndomain='ht'\n",
            serde_json::to_string(&root.path().to_string_lossy()).unwrap())).unwrap();
        let storage = vfs::OpendalFileStorage::with_mounts(
            root.path(),
            std::sync::Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap()),
        );
        let file = FileRef::remote(
            "vfs:///cluster/output.tsv",
            Some("tsv".into()),
            7,
            Some("abc".into()),
        );
        let upgraded = file.canonicalized(&storage).unwrap();
        assert_eq!(upgraded.path, "ht://output.tsv");
        assert_eq!(upgraded.domain.as_str(), "ht");
        assert_eq!(
            upgraded.fingerprint.unwrap().content_hash.as_deref(),
            Some("abc")
        );
        assert!(
            FileRef::new("vfs:///cluster/output.tsv", None)
                .in_domain(DomainId::new("other"))
                .canonicalized(&storage)
                .is_err()
        );
        assert_eq!(
            FileRef::new("/host/file.tsv", None)
                .canonicalized(&storage)
                .unwrap()
                .path,
            "/host/file.tsv"
        );
    }

    #[test]
    fn uri_domain_is_inferred_and_conflicts_are_rejected() {
        let file: FileRef = serde_json::from_str(r#"{"path":"ht://share/x.tsv"}"#).unwrap();
        assert_eq!(file.domain.as_str(), "ht");
        assert_eq!(FileRef::new("ht://share/x.tsv", None).domain, file.domain);
        assert!(
            serde_json::from_str::<FileRef>(r#"{"path":"ht://share/x.tsv","domain":"local"}"#)
                .is_err()
        );
        let artifact = ArtifactRef {
            uri: file.path,
            format: None,
            fingerprint: None,
            producer: None,
            generation: None,
        };
        assert_eq!(FileRef::from(artifact).domain.as_str(), "ht");
    }

    #[test]
    fn pre_domain_records_parse_as_local() {
        let legacy = r#"{"path":"/data/x.tsv","format":"tsv"}"#;
        let file: FileRef = serde_json::from_str(legacy).unwrap();
        assert!(file.domain.is_local());
        // And the local wire form is unchanged: no domain key is emitted.
        let round: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        assert!(round.get("domain").is_none());
    }

    #[test]
    fn named_domains_round_trip() {
        let mut file = FileRef::new("/data/x.tsv", None);
        assert!(file.domain.is_local());
        file = file.in_domain(DomainId::new("hpc-b"));
        let encoded = serde_json::to_string(&file).unwrap();
        assert!(encoded.contains("\"domain\":\"hpc-b\""), "{encoded}");
        let decoded: FileRef = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.domain.as_str(), "hpc-b");
        assert!(!decoded.domain.is_local());
    }

    #[test]
    fn unknown_domain_ids_are_preserved_verbatim() {
        let file: FileRef =
            serde_json::from_str(r#"{"path":"/x","domain":"future-cluster"}"#).unwrap();
        assert_eq!(file.domain.as_str(), "future-cluster");
    }
}
