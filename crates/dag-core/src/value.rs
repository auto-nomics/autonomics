//! Typed values carried by DAG edges.

use datafusion::prelude::DataFrame;
use serde::Serialize;

use crate::dag::DagError;

/// The runtime payload carried by one edge.
#[derive(Debug, Clone)]
pub enum NodeValue {
    DataFrame(DataFrame),
    File(FileRef),
    FileSet(Vec<FileRef>),
}

impl NodeValue {
    pub fn data_type(&self) -> PortType {
        match self {
            Self::DataFrame(_) => PortType::DataFrame,
            Self::File(_) => PortType::File,
            Self::FileSet(_) => PortType::FileSet,
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

/// A file artifact address passed between nodes.
#[derive(Debug, Clone, Serialize)]
pub struct FileRef {
    /// Absolute or engine-mounted path. Relative paths are not accepted at
    /// runtime because they depend on the caller's process working directory.
    pub path: String,
    /// Optional normalized format label (for example `csv`, `vcf`, `parquet`).
    pub format: Option<String>,
    pub fingerprint: Option<FileFingerprint>,
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
}

/// Cheap artifact identity suitable for incremental invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
}

/// The payload type declared by a node port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    DataFrame,
    File,
    FileSet,
    Any,
}

impl std::fmt::Display for PortType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DataFrame => "dataframe",
            Self::File => "file",
            Self::FileSet => "file_set",
            Self::Any => "any",
        })
    }
}

impl PortType {
    pub fn accepts(&self, actual: PortType) -> bool {
        *self == PortType::Any || actual == PortType::Any || *self == actual
    }
}
