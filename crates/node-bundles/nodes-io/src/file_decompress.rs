//! Streaming decompression for a single compressed File artifact.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileFingerprint, FileRef, PortType};

pub const FILE_DECOMPRESS_KIND: &str = "file_decompress";

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const COPY_CHUNK_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DecompressionFormat {
    #[default]
    Auto,
    Gzip,
    Bgzf,
    Zstd,
    Bzip2,
    Xz,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileDecompressSpec {
    /// Input path used when no upstream File is connected.
    #[serde(default)]
    pub src: Option<String>,
    /// Concrete output file path. Directories are not accepted.
    pub dst: String,
    /// Compression format. The default detects gzip/BGZF, zstd, bzip2, and xz
    /// from leading magic bytes and validates the path extension.
    #[serde(default)]
    pub format: DecompressionFormat,
    /// Refuse outputs larger than this many bytes. The default is 64 GiB.
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: u64,
}

fn default_max_output_bytes() -> u64 {
    DEFAULT_MAX_OUTPUT_BYTES
}

#[derive(Clone)]
pub struct FileDecompressNode {
    ports: NodePorts,
    spec: FileDecompressSpec,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_accepted_formats(
            None,
            PortType::File,
            "compressed_file",
            "gzip",
            [
                "gzip", "gz", "bgz", "bgzf", "zst", "zstd", "bz2", "xz", "tgz", "tar_gz",
            ],
        )
        .add_output_port_of_type(None, PortType::File)
}

impl FileDecompressNode {
    pub fn new(spec: FileDecompressSpec) -> Self {
        Self {
            ports: port_layout(),
            spec,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetectedFormat {
    Gzip,
    Zstd,
    Bzip2,
    Xz,
}

impl DetectedFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
            Self::Bzip2 => "bzip2",
            Self::Xz => "xz",
        }
    }
}

fn expected_format(path: &str, explicit: DecompressionFormat) -> Option<DetectedFormat> {
    match explicit {
        DecompressionFormat::Gzip | DecompressionFormat::Bgzf => Some(DetectedFormat::Gzip),
        DecompressionFormat::Zstd => Some(DetectedFormat::Zstd),
        DecompressionFormat::Bzip2 => Some(DetectedFormat::Bzip2),
        DecompressionFormat::Xz => Some(DetectedFormat::Xz),
        DecompressionFormat::Auto => extension_format(path),
    }
}

fn extension_format(path: &str) -> Option<DetectedFormat> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".gz") || lower.ends_with(".bgz") {
        Some(DetectedFormat::Gzip)
    } else if lower.ends_with(".zst") || lower.ends_with(".zstd") {
        Some(DetectedFormat::Zstd)
    } else if lower.ends_with(".bz2") {
        Some(DetectedFormat::Bzip2)
    } else if lower.ends_with(".xz") {
        Some(DetectedFormat::Xz)
    } else {
        None
    }
}

fn magic_format(prefix: &[u8]) -> Option<DetectedFormat> {
    if prefix.starts_with(&[0x1f, 0x8b]) {
        Some(DetectedFormat::Gzip)
    } else if prefix.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Some(DetectedFormat::Zstd)
    } else if prefix.starts_with(b"BZh") {
        Some(DetectedFormat::Bzip2)
    } else if prefix.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]) {
        Some(DetectedFormat::Xz)
    } else {
        None
    }
}

fn detect_format(path: &str, explicit: DecompressionFormat) -> Result<DetectedFormat, String> {
    let mut file = std::fs::File::open(path).map_err(|error| format!("open `{path}`: {error}"))?;
    let mut prefix = [0_u8; 6];
    let read = file
        .read(&mut prefix)
        .map_err(|error| format!("read magic bytes from `{path}`: {error}"))?;
    let detected = magic_format(&prefix[..read])
        .ok_or_else(|| format!("cannot detect compression format from `{path}` magic bytes"))?;

    if let Some(expected) = expected_format(path, explicit)
        && expected != detected
        && explicit != DecompressionFormat::Auto
    {
        return Err(format!(
            "input `{path}` looks like {} but was requested as {}",
            detected.label(),
            expected.label()
        ));
    }
    Ok(detected)
}

fn decoder(input: std::fs::File, format: DetectedFormat) -> Result<Box<dyn Read + Send>, String> {
    match format {
        DetectedFormat::Gzip => Ok(Box::new(flate2::read::MultiGzDecoder::new(input))),
        DetectedFormat::Zstd => zstd::Decoder::new(input)
            .map(|decoder| Box::new(decoder) as Box<dyn Read + Send>)
            .map_err(|error| error.to_string()),
        DetectedFormat::Bzip2 => Ok(Box::new(bzip2::read::MultiBzDecoder::new(input))),
        DetectedFormat::Xz => Ok(Box::new(liblzma::read::XzDecoder::new_multi_decoder(input))),
    }
}

fn decode_blocking(
    input: &Path,
    output: &Path,
    format: DetectedFormat,
    max_output_bytes: u64,
) -> Result<FileFingerprint, String> {
    let source = std::fs::File::open(input).map_err(|error| error.to_string())?;
    let mut reader = decoder(source, format).map_err(|error| error.to_string())?;
    let mut destination = std::fs::File::create(output).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut chunk = vec![0_u8; COPY_CHUNK_BYTES];

    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|error| format!("decompression failed after {size} bytes: {error}"))?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| "output size overflowed u64".to_string())?;
        if size > max_output_bytes {
            return Err(format!(
                "decompressed output exceeds max_output_bytes ({max_output_bytes})"
            ));
        }
        hasher.update(&chunk[..read]);
        destination
            .write_all(&chunk[..read])
            .map_err(|error| format!("write `{}`: {error}", output.display()))?;
    }
    destination
        .flush()
        .map_err(|error| format!("flush `{}`: {error}", output.display()))?;
    Ok(FileFingerprint {
        size,
        mtime_ns: 0,
        content_hash: Some(format!("sha256:{}", hex(&hasher.finalize()))),
        immutable_remote: false,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn format_from_path(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
}

async fn stage_vfs_input(ctx: &NodeCtx, source: &str, destination: &Path) -> Result<(), DagError> {
    let virtual_path = source
        .strip_prefix("vfs://")
        .ok_or_else(|| DagError::Schedule(format!("expected VFS path `{source}`")))?;
    let storage = ctx
        .opendal
        .as_ref()
        .ok_or_else(|| DagError::Schedule("VFS input requires mounted runtime VFS".into()))?;
    let length = storage
        .content_length(virtual_path)
        .await
        .map_err(|error| DagError::Schedule(format!("cannot stat VFS file `{source}`: {error}")))?;
    let mut stream = Box::pin(storage.read_stream(virtual_path, 0..length).await.map_err(
        |error| DagError::Schedule(format!("cannot open VFS file `{source}`: {error}")),
    )?);
    let mut staged = tokio::fs::File::create(destination)
        .await
        .map_err(|error| {
            DagError::Schedule(format!(
                "cannot create staging file `{}`: {error}",
                destination.display()
            ))
        })?;
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            DagError::Schedule(format!("cannot read VFS file `{source}`: {error}"))
        })?;
        staged.write_all(&chunk).await.map_err(|error| {
            DagError::Schedule(format!(
                "cannot write staging file `{}`: {error}",
                destination.display()
            ))
        })?;
    }
    staged.flush().await.map_err(|error| {
        DagError::Schedule(format!(
            "cannot flush staging file `{}`: {error}",
            destination.display()
        ))
    })?;
    Ok(())
}

fn normalize_and_source(ctx: &NodeCtx, path: &str) -> String {
    crate::file_to_dataframe::source_path(ctx, &crate::file_to_dataframe::normalize_path(path))
}

async fn upload_local(ctx: &NodeCtx, path: &str, local: &Path) -> Result<(), DagError> {
    let virtual_path = path
        .strip_prefix("vfs://")
        .ok_or_else(|| DagError::Schedule(format!("expected VFS path `{path}`")))?;
    let storage = ctx
        .opendal
        .as_ref()
        .ok_or_else(|| DagError::Schedule("VFS output requires mounted runtime VFS".into()))?;
    storage.check_writable(virtual_path).map_err(|error| {
        DagError::Schedule(format!("VFS path `{path}` is not writable: {error}"))
    })?;
    let object_path = storage.resolve_path(virtual_path);
    let operator = storage.resolve(virtual_path);
    let pending = format!(
        "{object_path}.pending-file-decompress-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let _ = operator.delete(&pending).await;

    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let source = local.to_path_buf();
    let hashing = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut input = std::fs::File::open(source).map_err(|error| error.to_string())?;
        let mut chunk = vec![0_u8; COPY_CHUNK_BYTES];
        loop {
            let read = input.read(&mut chunk).map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            if chunk_tx.blocking_send(chunk[..read].to_vec()).is_err() {
                break;
            }
        }
        Ok(())
    });

    let mut writer = operator.writer(&pending).await.map_err(|error| {
        DagError::Schedule(format!("cannot open VFS writer for `{path}`: {error}"))
    })?;
    let mut upload_result = Ok(());
    while let Some(chunk) = chunk_rx.recv().await {
        if let Err(error) = writer.write(chunk).await {
            upload_result = Err(DagError::Schedule(format!(
                "cannot upload decompressed output `{path}`: {error}"
            )));
            break;
        }
    }
    if upload_result.is_ok() {
        upload_result = writer.close().await.map(|_| ()).map_err(|error| {
            DagError::Schedule(format!("cannot finish VFS output `{path}`: {error}"))
        });
    }
    drop(chunk_rx);
    let hashing = hashing.await;
    if upload_result.is_ok()
        && let Err(error) = hashing
    {
        upload_result = Err(DagError::Schedule(error.to_string()));
    }
    if upload_result.is_err() {
        let _ = operator.delete(&pending).await;
        return upload_result;
    }

    let _ = operator.delete(&object_path).await;
    if let Err(error) = operator.rename(&pending, &object_path).await {
        if let Err(delete_error) = operator.delete(&pending).await {
            tracing::warn!(
                pending = %pending,
                error = %delete_error,
                "cannot remove pending decompressed output"
            );
        }
        return Err(DagError::Schedule(format!(
            "cannot publish VFS output `{path}`: {error}"
        )));
    }
    Ok(())
}

#[async_trait]
impl DagNode for FileDecompressNode {
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
        FILE_DECOMPRESS_KIND
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
        let upstream = inputs.first().and_then(|input| input.file_value().ok());
        let source = upstream
            .map(|file| file.path.clone())
            .or_else(|| self.spec.src.clone())
            .ok_or_else(|| {
                DagError::Schedule("file_decompress requires src or an upstream File".into())
            })?;
        let source = normalize_and_source(ctx, &source);
        let destination = normalize_and_source(ctx, &self.spec.dst);
        if source.trim().is_empty() || destination.trim().is_empty() {
            return Err(DagError::Schedule(
                "file_decompress src and dst cannot be empty".into(),
            ));
        }

        let workspace = tempfile::TempDir::new().map_err(|error| {
            DagError::Schedule(format!("cannot create decompression workspace: {error}"))
        })?;
        let staged_input = workspace.path().join("input.compressed");
        if source.starts_with("vfs://") {
            stage_vfs_input(ctx, &source, &staged_input).await?;
        } else if Path::new(&source).is_file() {
            tokio::fs::copy(&source, &staged_input)
                .await
                .map_err(|error| {
                    DagError::Schedule(format!("cannot stage input `{source}`: {error}"))
                })?;
        } else {
            return Err(DagError::Schedule(format!(
                "file_decompress input is not a regular file: `{source}`"
            )));
        }

        let format = detect_format(&staged_input.to_string_lossy(), self.spec.format)
            .map_err(DagError::Schedule)?;
        let local_parent = if destination.starts_with("vfs://") {
            None
        } else {
            Some(PathBuf::from(&destination))
        };
        let mut local_temp = None;
        let decoded = match &local_parent {
            Some(destination_path) => {
                let parent = destination_path.parent().ok_or_else(|| {
                    DagError::Schedule(format!("destination has no parent: `{destination}`"))
                })?;
                tokio::fs::create_dir_all(parent).await.map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot create output directory `{}`: {error}",
                        parent.display()
                    ))
                })?;
                let temp = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot create temporary output beside `{}`: {error}",
                        destination_path.display()
                    ))
                })?;
                let path = temp.path().to_path_buf();
                local_temp = Some(temp);
                path
            }
            None => workspace.path().join("decoded"),
        };

        let input_path = staged_input.clone();
        let output_path = decoded.clone();
        let max_bytes = self.spec.max_output_bytes;
        let mut fingerprint = tokio::task::spawn_blocking(move || {
            decode_blocking(&input_path, &output_path, format, max_bytes)
        })
        .await
        .map_err(|error| DagError::Schedule(format!("decompression task failed: {error}")))?
        .map_err(DagError::Schedule)?;

        if let Some(destination_path) = &local_parent
            && let Some(temp) = local_temp.take()
        {
            temp.persist(destination_path).map_err(|error| {
                DagError::Schedule(format!(
                    "cannot publish output `{}`: {error}",
                    destination_path.display()
                ))
            })?;
            let metadata = tokio::fs::metadata(destination_path)
                .await
                .map_err(|error| {
                    DagError::Schedule(format!(
                        "cannot stat output `{}`: {error}",
                        destination_path.display()
                    ))
                })?;
            fingerprint.mtime_ns = metadata
                .modified()
                .ok()
                .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos() as i128)
                .unwrap_or_default();
        } else {
            fingerprint.immutable_remote = true;
            upload_local(ctx, &destination, &decoded).await?;
        }

        let file = FileRef {
            path: destination.clone(),
            format: format_from_path(&destination),
            fingerprint: Some(fingerprint),
        };
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

pub struct FileDecompressNodeFactory;

impl NodeFactory for FileDecompressNodeFactory {
    fn kind(&self) -> &'static str {
        FILE_DECOMPRESS_KIND
    }

    fn desc(&self) -> &'static str {
        "Streaming decompression for gzip/BGZF, zstd, bzip2, and xz Files."
    }

    fn doc(&self) -> &'static str {
        "Reads one compressed File and emits one decompressed File. Format \
        detection uses magic bytes and validates an explicit format or known \
        extension. Decoding is streamed through a private workspace and bounded \
        by max_output_bytes (default 64 GiB). Local outputs are published by \
        rename and VFS outputs are uploaded to a pending object before being \
        published with a SHA-256 fingerprint."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FileDecompressSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: FileDecompressSpec = serde_json::from_value(spec)?;
        if spec.dst.trim().is_empty() {
            return Err("dst cannot be empty".into());
        }
        if spec.max_output_bytes == 0 {
            return Err("max_output_bytes must be greater than zero".into());
        }
        Ok(Box::new(FileDecompressNode::new(spec)))
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

    fn write_gzip(path: &Path, contents: &[u8]) {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(contents).unwrap();
        std::fs::write(path, encoder.finish().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn decompresses_gzip_with_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input.txt.gz");
        let destination = dir.path().join("output.txt");
        write_gzip(&source, b"hello compressed\n");

        let mut node = FileDecompressNode::new(FileDecompressSpec {
            src: Some(source.to_string_lossy().into_owned()),
            dst: destination.to_string_lossy().into_owned(),
            format: DecompressionFormat::Auto,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        });
        let outputs = node
            .execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let file = outputs.get(&0).unwrap().as_file().unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), b"hello compressed\n");
        assert_eq!(file.format.as_deref(), Some("txt"));
        assert_eq!(
            fingerprint_of(file),
            format!("sha256:{:x}", Sha256::digest(b"hello compressed\n"))
        );
    }

    #[tokio::test]
    async fn decompresses_all_single_stream_codecs() {
        let dir = tempfile::tempdir().unwrap();
        let expected = b"codec payload\n";
        let cases = [
            ("input.gz", {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(expected).unwrap();
                encoder.finish().unwrap()
            }),
            (
                "input.zst",
                zstd::stream::encode_all(&expected[..], 3).unwrap(),
            ),
            ("input.bz2", {
                let mut encoder =
                    bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
                encoder.write_all(expected).unwrap();
                encoder.finish().unwrap()
            }),
            ("input.xz", {
                let mut encoder = liblzma::write::XzEncoder::new(Vec::new(), 6);
                encoder.write_all(expected).unwrap();
                encoder.finish().unwrap()
            }),
        ];

        for (index, (name, compressed)) in cases.into_iter().enumerate() {
            let source = dir.path().join(name);
            let destination = dir.path().join(format!("output-{index}.txt"));
            std::fs::write(&source, compressed).unwrap();
            let mut node = FileDecompressNode::new(FileDecompressSpec {
                src: Some(source.to_string_lossy().into_owned()),
                dst: destination.to_string_lossy().into_owned(),
                format: DecompressionFormat::Auto,
                max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            });
            node.execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(std::fs::read(&destination).unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn rejects_output_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input.txt.gz");
        let destination = dir.path().join("output.txt");
        write_gzip(&source, b"0123456789");
        let mut node = FileDecompressNode::new(FileDecompressSpec {
            src: Some(source.to_string_lossy().into_owned()),
            dst: destination.to_string_lossy().into_owned(),
            format: DecompressionFormat::Auto,
            max_output_bytes: 3,
        });
        let error = node
            .execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("max_output_bytes"));
        assert!(!destination.exists());
    }

    fn fingerprint_of(file: &FileRef) -> String {
        file.fingerprint
            .as_ref()
            .and_then(|fingerprint| fingerprint.content_hash.clone())
            .unwrap()
    }

    #[test]
    fn factory_validates_limit() {
        let error = match FileDecompressNodeFactory.build(
            serde_json::json!({"src": "/in.gz", "dst": "/out", "max_output_bytes": 0}),
            ctx(),
        ) {
            Ok(_) => panic!("zero max_output_bytes must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("max_output_bytes"));
    }
}
