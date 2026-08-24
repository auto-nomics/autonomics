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
    Data(DataRef),
    DataSet(Vec<DataRef>),
}

impl NodeValue {
    pub fn data_type(&self) -> PortType {
        match self {
            Self::DataFrame(_) => PortType::DataFrame,
            Self::File(_) => PortType::File,
            Self::FileSet(_) => PortType::FileSet,
            Self::Data(_) => PortType::Data,
            Self::DataSet(_) => PortType::DataSet,
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

    pub fn as_data(&self) -> Result<&DataRef, DagError> {
        match self {
            Self::Data(data) => Ok(data),
            other => Err(DagError::Schedule(format!(
                "expected a Data value, got {}",
                other.data_type()
            ))),
        }
    }

    pub fn as_data_set(&self) -> Result<&[DataRef], DagError> {
        match self {
            Self::DataSet(data) => Ok(data),
            other => Err(DagError::Schedule(format!(
                "expected a DataSet value, got {}",
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

impl From<DataRef> for NodeValue {
    fn from(data: DataRef) -> Self {
        Self::Data(data)
    }
}

impl From<Vec<DataRef>> for NodeValue {
    fn from(data: Vec<DataRef>) -> Self {
        Self::DataSet(data)
    }
}

/// A file artifact address passed between nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRef {
    /// Absolute or engine-mounted path. Relative paths are not accepted at
    /// runtime because they depend on the caller's process working directory.
    pub path: String,
    /// Optional normalized format label (for example `csv`, `vcf`, `parquet`).
    pub format: Option<String>,
    pub fingerprint: Option<FileFingerprint>,
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
        Self {
            path: artifact.uri,
            format: artifact.format,
            fingerprint: artifact.fingerprint,
        }
    }
}

/// A logical artifact address passed between nodes.
///
/// Unlike [`FileRef`], a `DataRef` never claims that its path is present on
/// the execution host. Consumers either read it through the runtime VFS or
/// ask the runtime data plane to stage it first.
#[derive(Debug, Clone, Serialize)]
pub struct DataRef {
    /// Stable artifact identifier assigned by the data registry.
    pub artifact_id: String,
    /// Runtime virtual path resolved through the mounted object store.
    pub vpath: String,
    pub format: Option<String>,
    pub fingerprint: Option<FileFingerprint>,
    pub producer: Option<String>,
}

impl DataRef {
    pub fn new(vpath: impl Into<String>, format: Option<String>) -> Self {
        let vpath = vpath.into();
        Self {
            artifact_id: vpath.clone(),
            vpath,
            format,
            fingerprint: None,
            producer: None,
        }
    }

    pub fn from_file_ref(file: FileRef) -> Self {
        Self {
            artifact_id: file.path.clone(),
            vpath: file.path,
            format: file.format,
            fingerprint: file.fingerprint,
            producer: None,
        }
    }

    /// Compatibility view for legacy nodes that accept `FileRef`.
    ///
    /// The path remains virtual; such nodes must still stage it before using
    /// native filesystem tools.
    pub fn to_file_ref(&self) -> FileRef {
        FileRef {
            path: self.vpath.clone(),
            format: self.format.clone(),
            fingerprint: self.fingerprint.clone(),
        }
    }

    pub fn with_producer(mut self, producer: impl Into<String>) -> Self {
        self.producer = Some(producer.into());
        self
    }
}

impl FileRef {
    pub fn new(path: impl Into<String>, format: Option<String>) -> Self {
        Self {
            path: path.into(),
            format,
            fingerprint: None,
        }
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
        Self {
            path: uri.into(),
            format,
            fingerprint: Some(FileFingerprint::remote(size, sha256)),
        }
    }
}

/// Cheap artifact identity suitable for incremental invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub size: u64,
    pub mtime_ns: i128,
    pub content_hash: Option<String>,
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
    Data,
    DataSet,
    Any,
}

impl std::fmt::Display for PortType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DataFrame => "dataframe",
            Self::File => "file",
            Self::FileSet => "file_set",
            Self::Data => "data",
            Self::DataSet => "data_set",
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
mod tests {
    use super::*;

    #[test]
    fn data_refs_preserve_file_compatibility_metadata() {
        let file = FileRef {
            path: "/bundles/panels/panel.txt".into(),
            format: Some("txt".into()),
            fingerprint: Some(FileFingerprint {
                size: 10,
                mtime_ns: 20,
                content_hash: None,
            }),
        };

        let data = DataRef::from_file_ref(file.clone()).with_producer("source");
        let compatible = data.to_file_ref();

        assert_eq!(data.artifact_id, "/bundles/panels/panel.txt");
        assert_eq!(data.producer.as_deref(), Some("source"));
        assert_eq!(NodeValue::Data(data).data_type(), PortType::Data);
        assert_eq!(compatible.path, file.path);
        assert_eq!(compatible.format, file.format);
        assert_eq!(compatible.fingerprint, file.fingerprint);
    }
}
