//! Inspect and safely expand ZIP and TAR file archives.

use std::collections::BTreeSet;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{Array, BooleanArray, RecordBatch, StringArray, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::prelude::DataFrame;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileFingerprint, FileRef, PortType};

pub const ARCHIVE_INSPECT_KIND: &str = "archive_inspect";
pub const ARCHIVE_EXTRACT_KIND: &str = "archive_extract";

const DEFAULT_MAX_MEMBERS: usize = 100_000;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const DEFAULT_MAX_ENTRY_RATIO: u64 = 1_000;
const COPY_CHUNK_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    Zip,
    Tar(TarCompression),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TarCompression {
    None,
    Gzip,
    Zstd,
    Bzip2,
    Xz,
}

impl TarCompression {
    fn label(self) -> &'static str {
        match self {
            Self::None => "tar",
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
            Self::Bzip2 => "bzip2",
            Self::Xz => "xz",
        }
    }

    fn decoder(self, input: std::fs::File) -> Result<Box<dyn Read + Send>, String> {
        Ok(match self {
            Self::None => Box::new(input),
            Self::Gzip => Box::new(flate2::read::MultiGzDecoder::new(input)),
            Self::Zstd => Box::new(zstd::Decoder::new(input).map_err(|error| error.to_string())?),
            Self::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(input)),
            Self::Xz => Box::new(liblzma::read::XzDecoder::new_multi_decoder(input)),
        })
    }
}

fn detect_archive(path: &str) -> Result<ArchiveFormat, String> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        return Ok(ArchiveFormat::Zip);
    }
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") || lower.ends_with(".tar.bgz") {
        return Ok(ArchiveFormat::Tar(TarCompression::Gzip));
    }
    if lower.ends_with(".tar.zst") || lower.ends_with(".tzst") {
        return Ok(ArchiveFormat::Tar(TarCompression::Zstd));
    }
    if lower.ends_with(".tar.bz2") {
        return Ok(ArchiveFormat::Tar(TarCompression::Bzip2));
    }
    if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
        return Ok(ArchiveFormat::Tar(TarCompression::Xz));
    }
    if lower.ends_with(".tar") {
        return Ok(ArchiveFormat::Tar(TarCompression::None));
    }

    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut prefix = [0_u8; 6];
    let read = file.read(&mut prefix).map_err(|error| error.to_string())?;
    if prefix.starts_with(&[0x50, 0x4b, 0x03, 0x04]) {
        return Ok(ArchiveFormat::Zip);
    }
    if prefix.starts_with(&[0x1f, 0x8b]) {
        return Ok(ArchiveFormat::Tar(TarCompression::Gzip));
    }
    if prefix.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return Ok(ArchiveFormat::Tar(TarCompression::Zstd));
    }
    if prefix.starts_with(b"BZh") {
        return Ok(ArchiveFormat::Tar(TarCompression::Bzip2));
    }
    if prefix.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]) {
        return Ok(ArchiveFormat::Tar(TarCompression::Xz));
    }
    if read < prefix.len() {
        return Err(format!("cannot infer archive format from `{path}`"));
    }
    let mut tar_magic = [0_u8; 8];
    if file
        .seek(std::io::SeekFrom::Start(257))
        .and_then(|_| file.read_exact(&mut tar_magic))
        .is_ok()
        && (tar_magic.starts_with(b"ustar\0") || tar_magic.starts_with(b"GNUtar "))
    {
        return Ok(ArchiveFormat::Tar(TarCompression::None));
    }
    Err(format!("cannot infer archive format from `{path}`"))
}

fn normalize_member(name: &str) -> Result<String, String> {
    if name.is_empty() || name.contains('\0') {
        return Err(format!("invalid empty or NUL archive member `{name}`"));
    }
    let unified = name.replace('\\', "/");
    if unified.starts_with('/') {
        return Err(format!(
            "absolute archive member path is not allowed: `{name}`"
        ));
    }
    let mut components = Vec::new();
    for component in unified.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(format!(
                "archive member path may not contain `..`: `{name}`"
            ));
        }
        if components.is_empty()
            && component.len() >= 2
            && component.as_bytes()[1] == b':'
            && component.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(format!(
                "Windows drive archive member path is not allowed: `{name}`"
            ));
        }
        components.push(component);
    }
    if components.is_empty() {
        return Err(format!("archive member has no usable path: `{name}`"));
    }
    Ok(components.join("/"))
}

fn strip_components(path: &str, strip: usize) -> Option<String> {
    let stripped: Vec<_> = path.split('/').skip(strip).collect();
    (!stripped.is_empty()).then(|| stripped.join("/"))
}

fn compile_globs(patterns: &[String], field: &str) -> Result<Vec<glob::Pattern>, String> {
    patterns
        .iter()
        .map(|pattern| {
            glob::Pattern::new(pattern)
                .map_err(|error| format!("invalid {field} glob `{pattern}`: {error}"))
        })
        .collect()
}

fn selected(path: &str, include: &[glob::Pattern], exclude: &[glob::Pattern]) -> bool {
    (include.is_empty() || include.iter().any(|pattern| pattern.matches(path)))
        && !exclude.iter().any(|pattern| pattern.matches(path))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Directory,
    Link,
    Other,
}

impl EntryKind {
    fn label(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Link => "link",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
struct ArchiveEntry {
    path: String,
    kind: EntryKind,
    size: u64,
    compressed_size: Option<u64>,
    compression: Option<String>,
    crc32: Option<u32>,
    encrypted: bool,
}

fn inspect_zip(path: &Path, options: InspectOptions) -> Result<Vec<ArchiveEntry>, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    if archive.len() as usize > options.max_members {
        return Err(format!(
            "archive has more than max_members entries ({})",
            archive.len()
        ));
    }
    let mut entries = Vec::new();
    let mut paths = BTreeSet::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let path = normalize_member(entry.name())?;
        if !paths.insert(path.clone()) {
            return Err(format!("archive contains duplicate member `{path}`"));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| "archive total size overflowed u64".to_string())?;
        if total > options.max_total_bytes {
            return Err("archive declared size exceeds max_total_bytes".to_string());
        }
        let kind = if entry.is_dir() {
            EntryKind::Directory
        } else if entry.is_symlink() {
            EntryKind::Link
        } else if entry.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        entries.push(ArchiveEntry {
            path,
            kind,
            size: entry.size(),
            compressed_size: Some(entry.compressed_size()),
            compression: Some(entry.compression().to_string()),
            crc32: Some(entry.crc32()),
            encrypted: entry.encrypted(),
        });
    }
    Ok(entries)
}

fn inspect_tar(
    path: &Path,
    compression: TarCompression,
    options: InspectOptions,
) -> Result<Vec<ArchiveEntry>, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let reader = compression.decoder(file)?;
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(false);
    let mut entries = Vec::new();
    let mut paths = BTreeSet::new();
    let mut total = 0_u64;
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let raw_path = entry
            .path()
            .map_err(|error| error.to_string())?
            .to_str()
            .ok_or_else(|| "archive contains a non-UTF-8 member path".to_string())?
            .to_string();
        let path = normalize_member(&raw_path)?;
        if !paths.insert(path.clone()) {
            return Err(format!("archive contains duplicate member `{path}`"));
        }
        let header = entry.header();
        let entry_type = header.entry_type();
        let size = header.size().map_err(|error| error.to_string())?;
        total = total
            .checked_add(size)
            .ok_or_else(|| "archive total size overflowed u64".to_string())?;
        if total > options.max_total_bytes {
            return Err("archive declared size exceeds max_total_bytes".to_string());
        }
        let kind = if entry_type.is_dir() {
            EntryKind::Directory
        } else if entry_type.is_symlink() || entry_type.is_hard_link() {
            EntryKind::Link
        } else if entry_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        entries.push(ArchiveEntry {
            path,
            kind,
            size,
            compressed_size: None,
            compression: Some(compression.label().to_string()),
            crc32: None,
            encrypted: false,
        });
        if entries.len() > options.max_members {
            return Err(format!(
                "archive has more than max_members entries ({})",
                options.max_members
            ));
        }
    }
    Ok(entries)
}

#[derive(Debug, Clone, Copy)]
struct InspectOptions {
    max_members: usize,
    max_total_bytes: u64,
}

fn inspect_archive(
    path: &Path,
    format: ArchiveFormat,
    options: InspectOptions,
) -> Result<Vec<ArchiveEntry>, String> {
    match format {
        ArchiveFormat::Zip => inspect_zip(path, options),
        ArchiveFormat::Tar(compression) => inspect_tar(path, compression, options),
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ArchiveInspectSpec {
    #[serde(default)]
    pub src: Option<String>,
    #[serde(default = "default_max_members")]
    pub max_members: usize,
    #[serde(default = "default_max_total_bytes")]
    pub max_total_bytes: u64,
}

fn default_max_members() -> usize {
    DEFAULT_MAX_MEMBERS
}

fn default_max_total_bytes() -> u64 {
    DEFAULT_MAX_TOTAL_BYTES
}

fn default_max_entry_ratio() -> u64 {
    DEFAULT_MAX_ENTRY_RATIO
}

#[derive(Clone)]
pub struct ArchiveInspectNode {
    ports: NodePorts,
    spec: ArchiveInspectSpec,
}

impl ArchiveInspectNode {
    pub fn new(spec: ArchiveInspectSpec) -> Self {
        Self {
            ports: inspect_ports(),
            spec,
        }
    }
}

fn archive_input_ports() -> NodePorts {
    NodePorts::new().add_optional_input_port_of_type(PortType::File)
}

fn inspect_ports() -> NodePorts {
    archive_input_ports().add_output_port(Some(Arc::new(Schema::new(vec![
        Field::new("member_path", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("size", DataType::UInt64, false),
        Field::new("compressed_size", DataType::UInt64, true),
        Field::new("compression", DataType::Utf8, true),
        Field::new("crc32", DataType::UInt32, true),
        Field::new("encrypted", DataType::Boolean, false),
    ]))))
}

#[async_trait]
impl DagNode for ArchiveInspectNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            spec: self.spec.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        ARCHIVE_INSPECT_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let source = input_path(&self.spec.src, inputs, "archive_inspect")?;
        let source = crate::file_decompress::normalize_and_source(ctx, &source);
        let workspace = tempfile::TempDir::new().map_err(|error| {
            DagError::Schedule(format!("cannot create archive workspace: {error}"))
        })?;
        let staged = staged_path(workspace.path(), &source);
        stage_archive(ctx, &source, &staged).await?;
        let format = detect_archive(&staged.to_string_lossy()).map_err(DagError::Schedule)?;
        let options = InspectOptions {
            max_members: self.spec.max_members,
            max_total_bytes: self.spec.max_total_bytes,
        };
        let entries =
            tokio::task::spawn_blocking(move || inspect_archive(&staged, format, options))
                .await
                .map_err(|error| DagError::Schedule(error.to_string()))?
                .map_err(DagError::Schedule)?;
        let batch = entries_record_batch(&entries)?;
        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(DagError::DataFusion)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

fn entries_record_batch(entries: &[ArchiveEntry]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("member_path", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("size", DataType::UInt64, false),
        Field::new("compressed_size", DataType::UInt64, true),
        Field::new("compression", DataType::Utf8, true),
        Field::new("crc32", DataType::UInt32, true),
        Field::new("encrypted", DataType::Boolean, false),
    ]));
    let columns: Vec<Arc<dyn Array>> = vec![
        Arc::new(StringArray::from_iter(
            entries.iter().map(|entry| Some(entry.path.as_str())),
        )),
        Arc::new(StringArray::from_iter(
            entries.iter().map(|entry| Some(entry.kind.label())),
        )),
        Arc::new(UInt64Array::from_iter(
            entries.iter().map(|entry| entry.size),
        )),
        Arc::new(UInt64Array::from_iter(
            entries
                .iter()
                .map(|entry| entry.compressed_size)
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from_iter(
            entries
                .iter()
                .map(|entry| entry.compression.as_deref())
                .collect::<Vec<_>>(),
        )),
        Arc::new(UInt32Array::from_iter(
            entries.iter().map(|entry| entry.crc32).collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from_iter(
            entries.iter().map(|entry| entry.encrypted),
        )),
    ];
    RecordBatch::try_new(schema, columns)
        .map_err(|error| DagError::Schedule(format!("cannot build archive listing: {error}")))
}

pub struct ArchiveInspectNodeFactory;

impl NodeFactory for ArchiveInspectNodeFactory {
    fn kind(&self) -> &'static str {
        ARCHIVE_INSPECT_KIND
    }

    fn desc(&self) -> &'static str {
        "Lists ZIP and TAR members without extracting payloads."
    }

    fn doc(&self) -> &'static str {
        "Reads one ZIP or TAR/TAR-compressed File and emits a DataFrame with \
        member_path, kind, size, compressed_size, compression, crc32, and encrypted \
        fields. Member paths are normalized and unsafe paths are rejected. Listings \
        are bounded by max_members and max_total_bytes."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ArchiveInspectSpec)
    }

    fn ports(&self) -> NodePorts {
        inspect_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: ArchiveInspectSpec = serde_json::from_value(spec)?;
        if spec.max_members == 0 {
            return Err("max_members must be greater than zero".into());
        }
        if spec.max_total_bytes == 0 {
            return Err("max_total_bytes must be greater than zero".into());
        }
        Ok(Box::new(ArchiveInspectNode {
            ports: inspect_ports(),
            spec,
        }))
    }
}

fn input_path(src: &Option<String>, inputs: &[NodeInput], kind: &str) -> Result<String, DagError> {
    inputs
        .first()
        .and_then(|input| input.file_value().ok())
        .map(|file| file.path.clone())
        .or_else(|| src.clone())
        .ok_or_else(|| DagError::Schedule(format!("{kind} requires src or an upstream File")))
}

fn staged_path(root: &Path, source: &str) -> PathBuf {
    let name = Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("archive");
    root.join(name)
}

async fn stage_archive(ctx: &NodeCtx, source: &str, staged: &Path) -> Result<(), DagError> {
    if source.starts_with("vfs://") {
        crate::file_decompress::stage_vfs_input(ctx, source, staged).await
    } else if Path::new(source).is_file() {
        tokio::fs::copy(source, staged)
            .await
            .map(|_| ())
            .map_err(|error| DagError::Schedule(format!("cannot stage `{source}`: {error}")))
    } else {
        Err(DagError::Schedule(format!(
            "archive input is not a regular file: `{source}`"
        )))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ArchiveExtractSpec {
    #[serde(default)]
    pub src: Option<String>,
    pub destination_prefix: String,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub strip_components: usize,
    #[serde(default = "default_max_members")]
    pub max_members: usize,
    #[serde(default = "default_max_total_bytes")]
    pub max_total_bytes: u64,
    #[serde(default = "default_max_entry_ratio")]
    pub max_entry_ratio: u64,
}

impl Default for ArchiveExtractSpec {
    fn default() -> Self {
        Self {
            src: None,
            destination_prefix: String::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            strip_components: 0,
            max_members: DEFAULT_MAX_MEMBERS,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_entry_ratio: DEFAULT_MAX_ENTRY_RATIO,
        }
    }
}

#[derive(Debug)]
struct ExtractedFile {
    path: String,
    local_path: PathBuf,
    fingerprint: FileFingerprint,
}

fn extract_ports() -> NodePorts {
    archive_input_ports().add_output_port_of_type(None, PortType::FileSet)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn ratio_exceeds(size: u128, compressed_size: Option<u64>, ratio: u64) -> bool {
    compressed_size.is_some_and(|compressed| {
        (compressed == 0 && size > 0)
            || (compressed > 0 && size > (compressed as u128).saturating_mul(ratio as u128))
    })
}

fn copy_member<R: Read, W: Write>(
    mut reader: R,
    mut writer: W,
    entry_limit: u64,
    max_entry_ratio: u64,
    compressed_size: Option<u64>,
) -> Result<FileFingerprint, String> {
    if ratio_exceeds(entry_limit as u128, compressed_size, max_entry_ratio) {
        return Err(format!(
            "entry expansion exceeds max_entry_ratio ({max_entry_ratio})"
        ));
    }
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut chunk = vec![0_u8; COPY_CHUNK_BYTES];
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|error| format!("archive member read failed after {size} bytes: {error}"))?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| "archive member size overflowed u64".to_string())?;
        if size > entry_limit {
            return Err("archive member exceeds its declared or remaining size".to_string());
        }
        if ratio_exceeds(size as u128, compressed_size, max_entry_ratio) {
            return Err(format!(
                "entry expansion exceeds max_entry_ratio ({max_entry_ratio})"
            ));
        }
        hasher.update(&chunk[..read]);
        writer
            .write_all(&chunk[..read])
            .map_err(|error| format!("archive member write failed: {error}"))?;
    }
    writer.flush().map_err(|error| error.to_string())?;
    Ok(FileFingerprint {
        size,
        mtime_ns: 0,
        content_hash: Some(format!("sha256:{}", hex(&hasher.finalize()))),
        immutable_remote: false,
    })
}

struct ExtractOptions {
    include: Vec<glob::Pattern>,
    exclude: Vec<glob::Pattern>,
    strip_components: usize,
    max_members: usize,
    max_total_bytes: u64,
    max_entry_ratio: u64,
}

fn write_member(
    root: &Path,
    path: &str,
    reader: impl Read,
    declared_size: u64,
    compressed_size: Option<u64>,
    entry_limit: u64,
    options: &ExtractOptions,
) -> Result<ExtractedFile, String> {
    let destination = root.join(path);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let output = std::fs::File::create(&destination).map_err(|error| error.to_string())?;
    let fingerprint = copy_member(
        reader,
        output,
        entry_limit,
        options.max_entry_ratio,
        compressed_size,
    )?;
    if fingerprint.size != declared_size {
        return Err(format!(
            "archive member `{path}` declared {declared_size} bytes but yielded {}",
            fingerprint.size
        ));
    }
    Ok(ExtractedFile {
        path: path.to_string(),
        local_path: destination,
        fingerprint,
    })
}

fn extract_zip(
    path: &Path,
    root: &Path,
    options: &ExtractOptions,
) -> Result<Vec<ExtractedFile>, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let mut paths = BTreeSet::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let normalized = normalize_member(entry.name())?;
        if !selected(&normalized, &options.include, &options.exclude) {
            continue;
        }
        let Some(path) = strip_components(&normalized, options.strip_components) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        if entry.is_symlink() || !entry.is_file() {
            return Err(format!("unsupported ZIP member type: `{normalized}`"));
        }
        if entry.encrypted() {
            return Err(format!(
                "encrypted archive member is not supported: `{normalized}`"
            ));
        }
        if !matches!(
            entry.compression(),
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
        ) {
            return Err(format!(
                "unsupported ZIP compression `{}` for `{normalized}`",
                entry.compression()
            ));
        }
        if !paths.insert(path.clone()) {
            return Err(format!(
                "archive extraction produced duplicate path `{path}`"
            ));
        }
        if files.len() >= options.max_members {
            return Err(format!(
                "archive extraction exceeds max_members ({})",
                options.max_members
            ));
        }
        let size = entry.size();
        total = total
            .checked_add(size)
            .ok_or_else(|| "archive total size overflowed u64".to_string())?;
        if total > options.max_total_bytes {
            return Err("archive expansion exceeds max_total_bytes".to_string());
        }
        let compressed_size = Some(entry.compressed_size());
        files.push(write_member(
            root,
            &path,
            &mut entry,
            size,
            compressed_size,
            size,
            options,
        )?);
    }
    Ok(files)
}

fn extract_tar(
    path: &Path,
    compression: TarCompression,
    root: &Path,
    options: &ExtractOptions,
) -> Result<Vec<ExtractedFile>, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let reader = compression.decoder(file)?;
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(false);
    let mut paths = BTreeSet::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let raw_path = entry
            .path()
            .map_err(|error| error.to_string())?
            .to_str()
            .ok_or_else(|| "archive contains a non-UTF-8 member path".to_string())?
            .to_string();
        let normalized = normalize_member(&raw_path)?;
        if !selected(&normalized, &options.include, &options.exclude) {
            continue;
        }
        let Some(path) = strip_components(&normalized, options.strip_components) else {
            continue;
        };
        let header = entry.header();
        let entry_type = header.entry_type();
        if entry_type.is_dir() {
            continue;
        }
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(format!("archive links are not supported: `{normalized}`"));
        }
        if !entry_type.is_file() {
            return Err(format!("unsupported TAR member type: `{normalized}`"));
        }
        if !paths.insert(path.clone()) {
            return Err(format!(
                "archive extraction produced duplicate path `{path}`"
            ));
        }
        if files.len() >= options.max_members {
            return Err(format!(
                "archive extraction exceeds max_members ({})",
                options.max_members
            ));
        }
        let size = header.size().map_err(|error| error.to_string())?;
        total = total
            .checked_add(size)
            .ok_or_else(|| "archive total size overflowed u64".to_string())?;
        if total > options.max_total_bytes {
            return Err("archive expansion exceeds max_total_bytes".to_string());
        }
        files.push(write_member(
            root, &path, &mut entry, size, None, size, options,
        )?);
    }
    Ok(files)
}

fn extract_archive(
    path: &Path,
    format: ArchiveFormat,
    root: &Path,
    options: &ExtractOptions,
) -> Result<Vec<ExtractedFile>, String> {
    match format {
        ArchiveFormat::Zip => extract_zip(path, root, options),
        ArchiveFormat::Tar(compression) => extract_tar(path, compression, root, options),
    }
}

#[derive(Clone)]
pub struct ArchiveExtractNode {
    ports: NodePorts,
    spec: ArchiveExtractSpec,
}

impl ArchiveExtractNode {
    pub fn new(spec: ArchiveExtractSpec) -> Self {
        Self {
            ports: extract_ports(),
            spec,
        }
    }
}

fn virtual_join(prefix: &str, member: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    if prefix == "vfs://" {
        format!("vfs:///{member}")
    } else {
        format!("{prefix}/{member}")
    }
}

async fn delete_vfs_outputs(ctx: &NodeCtx, paths: &[String]) {
    let Some(storage) = ctx.opendal.as_ref() else {
        return;
    };
    for path in paths {
        let Some(virtual_path) = path.strip_prefix("vfs://") else {
            continue;
        };
        let operator = storage.resolve(virtual_path);
        let _ = operator.delete(&storage.resolve_path(virtual_path)).await;
    }
}

#[async_trait]
impl DagNode for ArchiveExtractNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            spec: self.spec.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        ARCHIVE_EXTRACT_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let source = input_path(&self.spec.src, inputs, "archive_extract")?;
        let source = crate::file_decompress::normalize_and_source(ctx, &source);
        let destination_prefix =
            crate::file_decompress::normalize_and_source(ctx, &self.spec.destination_prefix);
        let workspace = tempfile::TempDir::new().map_err(|error| {
            DagError::Schedule(format!("cannot create archive workspace: {error}"))
        })?;
        let staged = staged_path(workspace.path(), &source);
        stage_archive(ctx, &source, &staged).await?;
        let format = detect_archive(&staged.to_string_lossy()).map_err(DagError::Schedule)?;
        let options = ExtractOptions {
            include: compile_globs(&self.spec.include, "include").map_err(DagError::Schedule)?,
            exclude: compile_globs(&self.spec.exclude, "exclude").map_err(DagError::Schedule)?,
            strip_components: self.spec.strip_components,
            max_members: self.spec.max_members,
            max_total_bytes: self.spec.max_total_bytes,
            max_entry_ratio: self.spec.max_entry_ratio,
        };
        let extracted_root = workspace.path().join("extracted");
        std::fs::create_dir_all(&extracted_root)
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let files = tokio::task::spawn_blocking(move || {
            extract_archive(&staged, format, &extracted_root, &options)
        })
        .await
        .map_err(|error| DagError::Schedule(error.to_string()))?
        .map_err(DagError::Schedule)?;
        if files.is_empty() {
            return Err(DagError::Schedule(
                "archive_extract selected no file members".into(),
            ));
        }

        let mut outputs = Vec::new();
        let mut published = Vec::new();
        let mut result: Result<(), DagError> = Ok(());
        if destination_prefix.starts_with("vfs://") {
            for extracted in files {
                let path = virtual_join(&destination_prefix, &extracted.path);
                // Already-published VFS members must be removed before returning.
                #[allow(clippy::question_mark)]
                if let Err(error) =
                    crate::file_decompress::upload_local(ctx, &path, &extracted.local_path).await
                {
                    return Err(error);
                }
                published.push(path.clone());
                let mut fingerprint = extracted.fingerprint;
                fingerprint.immutable_remote = true;
                outputs.push(FileRef {
                    format: crate::file_decompress::format_from_path(&path),
                    path,
                    fingerprint: Some(fingerprint),
                });
            }
            if result.is_err() {
                delete_vfs_outputs(ctx, &published).await;
                return Err(result.expect_err("checked result above"));
            }
        } else {
            let root = PathBuf::from(&destination_prefix);
            if let Ok(metadata) = tokio::fs::metadata(&root).await
                && !metadata.is_dir()
            {
                return Err(DagError::Schedule(format!(
                    "archive_extract destination is not a directory: `{destination_prefix}`"
                )));
            }
            tokio::fs::create_dir_all(&root).await.map_err(|error| {
                DagError::Schedule(format!(
                    "cannot create archive destination `{}`: {error}",
                    root.display()
                ))
            })?;
            for extracted in files {
                let destination = root.join(&extracted.path);
                if let Some(parent) = destination.parent()
                    && let Err(error) = tokio::fs::create_dir_all(parent).await
                {
                    result = Err(DagError::Schedule(format!(
                        "cannot create `{}`: {error}",
                        parent.display()
                    )));
                    break;
                }
                if let Err(error) = tokio::fs::copy(&extracted.local_path, &destination).await {
                    result = Err(DagError::Schedule(format!(
                        "cannot publish `{}`: {error}",
                        destination.display()
                    )));
                    break;
                }
                let metadata = match tokio::fs::metadata(&destination).await {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        result = Err(DagError::Schedule(format!(
                            "cannot stat `{}`: {error}",
                            destination.display()
                        )));
                        break;
                    }
                };
                let mut fingerprint = extracted.fingerprint;
                fingerprint.mtime_ns = metadata
                    .modified()
                    .ok()
                    .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as i128)
                    .unwrap_or_default();
                let path = destination.to_string_lossy().into_owned();
                published.push(path.clone());
                outputs.push(FileRef {
                    format: crate::file_decompress::format_from_path(&path),
                    path,
                    fingerprint: Some(fingerprint),
                });
            }
            if result.is_err() {
                for path in published {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                return Err(result.expect_err("checked result above"));
            }
        }

        let mut port_outputs = PortOutputs::new();
        port_outputs.insert(0, outputs);
        Ok(port_outputs)
    }
}

pub struct ArchiveExtractNodeFactory;

impl NodeFactory for ArchiveExtractNodeFactory {
    fn kind(&self) -> &'static str {
        ARCHIVE_EXTRACT_KIND
    }

    fn desc(&self) -> &'static str {
        "Safely expands ZIP and TAR archives into a FileSet."
    }

    fn doc(&self) -> &'static str {
        "Extracts regular-file members from ZIP (stored/deflate) and \
        TAR/TAR.GZ/TAR.ZST/TAR.BZ2/TAR.XZ into destination_prefix. Include and \
        exclude globs, strip_components, member/total-size limits, and a per-entry \
        expansion ratio are enforced. Links, special files, absolute paths, `..`, \
        duplicate normalized paths, and encrypted ZIP entries are rejected. Output \
        is an ordered FileSet with SHA-256 fingerprints."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ArchiveExtractSpec)
    }

    fn ports(&self) -> NodePorts {
        extract_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: ArchiveExtractSpec = serde_json::from_value(spec)?;
        if spec.destination_prefix.trim().is_empty() {
            return Err("destination_prefix cannot be empty".into());
        }
        if spec.max_members == 0 {
            return Err("max_members must be greater than zero".into());
        }
        if spec.max_total_bytes == 0 {
            return Err("max_total_bytes must be greater than zero".into());
        }
        if spec.max_entry_ratio == 0 {
            return Err("max_entry_ratio must be greater than zero".into());
        }
        if let Err(error) = compile_globs(&spec.include, "include") {
            return Err(error.as_str().into());
        }
        if let Err(error) = compile_globs(&spec.exclude, "exclude") {
            return Err(error.as_str().into());
        }
        Ok(Box::new(ArchiveExtractNode {
            ports: extract_ports(),
            spec,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    fn mounted_ctx(root: &Path) -> (std::sync::Arc<vfs::OpendalFileStorage>, tempfile::TempDir) {
        let workspace = tempfile::tempdir().unwrap();
        let manifest = vfs::VfsManifest {
            backend: vec![vfs::BackendDefinition {
                id: "workspace".into(),
                config: vfs::BackendConfig::local("/"),
            }],
            mount: vec![vfs::MountDefinition {
                path: "/".into(),
                backend: "workspace".into(),
                source: root.to_string_lossy().into_owned(),
                read_only: false,
            }],
        };
        let mounted = vfs::MountedObjectStore::from_manifest(&manifest).unwrap();
        let storage = std::sync::Arc::new(vfs::OpendalFileStorage::with_mounts(
            workspace.path(),
            std::sync::Arc::new(mounted),
        ));
        (storage, workspace)
    }

    fn write_zip(path: &Path, members: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, contents) in members {
            archive.start_file(*name, options).unwrap();
            archive.write_all(contents).unwrap();
        }
        archive.finish().unwrap();
    }

    fn write_tar_gz(path: &Path, members: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (name, contents) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, *name, &contents[..])
                .unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn normalizes_unsafe_member_paths() {
        for path in [
            "../escape.txt",
            "/escape.txt",
            "C:\\escape.txt",
            "..\\escape.txt",
        ] {
            assert!(normalize_member(path).is_err(), "{path} must be rejected");
        }
        assert_eq!(normalize_member("./safe/a//b.txt").unwrap(), "safe/a/b.txt");
    }

    #[tokio::test]
    async fn inspects_zip_without_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.zip");
        write_zip(&path, &[("root/a.txt", b"alpha\n")]);
        let mut node = ArchiveInspectNode::new(ArchiveInspectSpec {
            src: Some(path.to_string_lossy().into_owned()),
            max_members: DEFAULT_MAX_MEMBERS,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        });
        let outputs = node
            .execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = outputs.get(&0).unwrap().as_dataframe().unwrap().clone();
        let batch = df.collect().await.unwrap().remove(0);
        let paths = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(paths.value(0), "root/a.txt");
    }

    #[tokio::test]
    async fn extracts_zip_with_filters_and_strip() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("input.zip");
        write_zip(
            &archive_path,
            &[
                ("root/a.txt", b"alpha\n"),
                ("root/nested/b.csv", b"id\n1\n"),
                ("ignored.txt", b"ignore\n"),
            ],
        );
        let destination = dir.path().join("extracted");
        let mut node = ArchiveExtractNode::new(ArchiveExtractSpec {
            src: Some(archive_path.to_string_lossy().into_owned()),
            destination_prefix: destination.to_string_lossy().into_owned(),
            include: vec!["root/**".into()],
            exclude: vec!["**/ignored.txt".into()],
            strip_components: 1,
            ..ArchiveExtractSpec::default()
        });
        let outputs = node
            .execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let files = outputs.get(&0).unwrap().as_file_set().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, destination.join("a.txt").to_string_lossy());
        assert_eq!(std::fs::read(&files[0].path).unwrap(), b"alpha\n");
        assert_eq!(
            files[1].path,
            destination.join("nested/b.csv").to_string_lossy()
        );
        assert_eq!(files[1].format.as_deref(), Some("csv"));
    }

    #[tokio::test]
    async fn extracts_tar_gz() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("input.tar.gz");
        write_tar_gz(&archive_path, &[("./dataset/table.tsv", b"a\tb\n")]);
        let destination = dir.path().join("out");
        let mut node = ArchiveExtractNode::new(ArchiveExtractSpec {
            src: Some(archive_path.to_string_lossy().into_owned()),
            destination_prefix: destination.to_string_lossy().into_owned(),
            ..ArchiveExtractSpec::default()
        });
        let outputs = node
            .execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let files = outputs.get(&0).unwrap().as_file_set().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path,
            destination.join("dataset/table.tsv").to_string_lossy()
        );
        assert_eq!(std::fs::read(&files[0].path).unwrap(), b"a\tb\n");
    }

    #[tokio::test]
    async fn extracts_all_supported_tar_compressions() {
        let dir = tempfile::tempdir().unwrap();
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut builder = tar::Builder::new(&mut cursor);
        let contents = b"payload\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "data.txt", &contents[..])
            .unwrap();
        builder.into_inner().unwrap();
        let plain_tar = cursor.into_inner();

        let cases = [
            (
                "input.tar.zst",
                zstd::stream::encode_all(&plain_tar[..], 3).unwrap(),
            ),
            ("input.tar.bz2", {
                let mut encoder =
                    bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
                encoder.write_all(&plain_tar).unwrap();
                encoder.finish().unwrap()
            }),
            ("input.tar.xz", {
                let mut encoder = liblzma::write::XzEncoder::new(Vec::new(), 6);
                encoder.write_all(&plain_tar).unwrap();
                encoder.finish().unwrap()
            }),
        ];
        for (index, (name, compressed)) in cases.into_iter().enumerate() {
            let archive_path = dir.path().join(name);
            std::fs::write(&archive_path, compressed).unwrap();
            let destination = dir.path().join(format!("out-{index}"));
            let mut node = ArchiveExtractNode::new(ArchiveExtractSpec {
                src: Some(archive_path.to_string_lossy().into_owned()),
                destination_prefix: destination.to_string_lossy().into_owned(),
                ..ArchiveExtractSpec::default()
            });
            node.execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(
                std::fs::read(destination.join("data.txt")).unwrap(),
                b"payload\n"
            );
        }
    }

    #[tokio::test]
    async fn extracts_zip_to_vfs_with_fingerprint() {
        let root = tempfile::tempdir().unwrap();
        let archive_path = root.path().join("input.zip");
        write_zip(&archive_path, &[("data/a.txt", b"vfs payload\n")]);
        let (storage, _workspace) = mounted_ctx(root.path());
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage.clone()),
        );
        let mut node = ArchiveExtractNode::new(ArchiveExtractSpec {
            src: Some("vfs:///input.zip".into()),
            destination_prefix: "vfs:///extracted".into(),
            ..ArchiveExtractSpec::default()
        });
        let outputs = node
            .execute(&ctx, &[], &dag_core::dag::node_event::NodeReporter::noop())
            .await
            .unwrap();
        let files = outputs.get(&0).unwrap().as_file_set().unwrap();
        assert_eq!(files[0].path, "vfs:///extracted/data/a.txt");

        let operator = storage.resolve("/extracted/data/a.txt");
        let contents = operator
            .read(&storage.resolve_path("/extracted/data/a.txt"))
            .await
            .unwrap();
        assert_eq!(contents.to_vec(), b"vfs payload\n");
        let fingerprint = files[0].fingerprint.as_ref().unwrap();
        assert_eq!(fingerprint.size, b"vfs payload\n".len() as u64);
        assert!(fingerprint.immutable_remote);
        assert!(
            fingerprint
                .content_hash
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
    }

    #[tokio::test]
    async fn rejects_tar_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("links.tar");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_path("link").unwrap();
        header.set_link_name("target").unwrap();
        header.set_cksum();
        archive.append(&header, std::io::empty()).unwrap();
        archive.into_inner().unwrap();

        let mut node = ArchiveExtractNode::new(ArchiveExtractSpec {
            src: Some(archive_path.to_string_lossy().into_owned()),
            destination_prefix: dir.path().join("safe").to_string_lossy().into_owned(),
            ..ArchiveExtractSpec::default()
        });
        let error = node
            .execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("links are not supported"));
    }
}
