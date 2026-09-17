//! File-level transform node for local and VFS-backed text artifacts.

use async_trait::async_trait;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::{DeflateEncoder, GzEncoder};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::io::{Read, Write};

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileRef, PortType};

pub const FILE_TRANSFORM_KIND: &str = "file_transform";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FileTransformOp {
    Gunzip,
    Gzip,
    Convert,
    Truncate,
    Transcode,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileTransformSpec {
    pub op: FileTransformOp,
    #[serde(default)]
    pub src: Option<String>,
    pub dst: String,
    #[serde(default)]
    pub format_in: Option<String>,
    #[serde(default)]
    pub format_out: Option<String>,
    #[serde(default)]
    pub delimiter: Option<String>,
    #[serde(default = "default_utf8")]
    pub encoding_in: String,
    #[serde(default = "default_utf8")]
    pub encoding_out: String,
    #[serde(default)]
    pub has_header: Option<bool>,
    #[serde(default)]
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub max_lines: Option<u64>,
}

fn default_utf8() -> String {
    "utf-8".into()
}

pub struct FileTransformNode {
    ports: NodePorts,
    src: Option<String>,
    dst: String,
    spec: FileTransformSpec,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_optional_input_port_of_type(PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

fn parse_delimiter(value: &str) -> Result<u8, String> {
    match value {
        "\\t" | "\t" | "tab" => Ok(b'\t'),
        "\\0" | "\0" | "nul" => Ok(0),
        "," | "comma" => Ok(b','),
        ";" | "semicolon" => Ok(b';'),
        "|" | "pipe" => Ok(b'|'),
        _ if value.len() == 1 && value.is_ascii() => Ok(value.as_bytes()[0]),
        _ => Err(format!(
            "delimiter must be one ASCII byte or tab/comma/semicolon/pipe; got `{value}`"
        )),
    }
}

fn normalize_and_source(ctx: &NodeCtx, path: &str) -> String {
    crate::file_to_dataframe::source_path(ctx, &crate::file_to_dataframe::normalize_path(path))
}

async fn read_bytes(ctx: &NodeCtx, path: &str) -> Result<Vec<u8>, DagError> {
    if let Some(virtual_path) = path.strip_prefix("vfs://") {
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule(format!("VFS input `{path}` requires a mounted runtime VFS"))
        })?;
        let operator = storage.resolve(virtual_path);
        return operator
            .read(&storage.resolve_path(virtual_path))
            .await
            .map(|buffer| buffer.to_vec())
            .map_err(|error| {
                DagError::Schedule(format!("cannot read VFS file `{path}`: {error}"))
            });
    }

    tokio::fs::read(path)
        .await
        .map_err(|error| DagError::Schedule(format!("cannot read file `{path}`: {error}")))
}

async fn write_bytes(ctx: &NodeCtx, path: &str, bytes: Vec<u8>) -> Result<(), DagError> {
    if let Some(virtual_path) = path.strip_prefix("vfs://") {
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule(format!(
                "VFS output `{path}` requires a mounted runtime VFS"
            ))
        })?;
        let operator = storage.resolve(virtual_path);
        return operator
            .write(&storage.resolve_path(virtual_path), bytes)
            .await
            .map_err(|error| DagError::Schedule(format!("cannot write VFS file `{path}`: {error}")))
            .map(|_| ());
    }

    let parent = std::path::Path::new(path)
        .parent()
        .ok_or_else(|| DagError::Schedule(format!("destination has no parent: `{path}`")))?;
    tokio::fs::create_dir_all(parent).await.map_err(|error| {
        DagError::Schedule(format!("cannot create `{}`: {error}", parent.display()))
    })?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| DagError::Schedule(format!("cannot write file `{path}`: {error}")))
}

fn is_gzip(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".gz") || lower.ends_with(".bgz")
}

fn decompress_if_gzip(path: &str, bytes: Vec<u8>) -> Result<Vec<u8>, DagError> {
    if !is_gzip(path) {
        return Ok(bytes);
    }
    let mut decoder = GzDecoder::new(&bytes[..]);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| DagError::Schedule(format!("invalid gzip input `{path}`: {error}")))?;
    Ok(output)
}

fn compress_gzip(bytes: Vec<u8>) -> Result<Vec<u8>, DagError> {
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&bytes)
        .and_then(|_| encoder.finish())
        .map_err(|error| DagError::Schedule(format!("cannot gzip output: {error}")))
}

fn compress_bgzf(bytes: Vec<u8>) -> Result<Vec<u8>, DagError> {
    const EOF_BLOCK: [u8; 28] = [
        0x1f, 0x8b, 0x08, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x06, 0x00, 0x42, 0x43, 0x02,
        0x00, 0x1b, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut output = Vec::new();
    for chunk in bytes.chunks(64 * 1024 - 64) {
        let mut deflater = DeflateEncoder::new(Vec::new(), Compression::default());
        let raw = deflater
            .write_all(chunk)
            .and_then(|_| deflater.finish())
            .map_err(|error| DagError::Schedule(format!("cannot deflate BGZF payload: {error}")))?;
        let block_size = raw.len() + 18 + 8;
        if block_size > u16::MAX as usize {
            return Err(DagError::Schedule(
                "BGZF block exceeds the 64KiB container limit".into(),
            ));
        }
        let mut crc = flate2::Crc::new();
        crc.update(chunk);
        let encoded_block_size = block_size as u16 - 1;
        output.extend_from_slice(&[
            0x1f,
            0x8b,
            0x08,
            0x04,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0xff,
            0x06,
            0x00,
            b'B',
            b'C',
            0x02,
            0x00,
            (encoded_block_size & 0xff) as u8,
            (encoded_block_size >> 8) as u8,
        ]);
        output.extend_from_slice(&raw);
        output.extend_from_slice(&crc.sum().to_le_bytes());
        output.extend_from_slice(&crc.amount().to_le_bytes());
    }
    output.extend_from_slice(&EOF_BLOCK);
    Ok(output)
}

fn compress_for_destination(path: &str, bytes: Vec<u8>) -> Result<Vec<u8>, DagError> {
    if path.to_ascii_lowercase().ends_with(".bgz") {
        compress_bgzf(bytes)
    } else {
        compress_gzip(bytes)
    }
}

fn encoding(label: &str) -> Result<&'static encoding_rs::Encoding, DagError> {
    encoding_rs::Encoding::for_label(label.as_bytes()).ok_or_else(|| {
        DagError::Schedule(format!(
            "unknown encoding `{label}`; examples: utf-8, utf-8-sig, gbk, latin1"
        ))
    })
}

fn decode(label: &str, bytes: &[u8]) -> Result<String, DagError> {
    let (text, _, had_errors) = encoding(label)?.decode(bytes);
    if had_errors {
        return Err(DagError::Schedule(format!(
            "input is not valid `{label}` text"
        )));
    }
    Ok(text.into_owned())
}

fn encode(label: &str, text: &str) -> Result<Vec<u8>, DagError> {
    Ok(encoding(label)?.encode(text).0.into_owned())
}

fn default_delimiter(path: &str, explicit_format: Option<&str>) -> u8 {
    let format = explicit_format
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| {
            std::path::Path::new(path)
                .extension()
                .and_then(|value| value.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default()
        });
    if format.contains("tsv") || path.to_ascii_lowercase().ends_with(".tsv") {
        b'\t'
    } else {
        b','
    }
}

fn convert_delimited(
    bytes: Vec<u8>,
    input_delimiter: u8,
    output_delimiter: u8,
    has_header: bool,
) -> Result<Vec<u8>, DagError> {
    let text = String::from_utf8(bytes)
        .map_err(|_| DagError::Schedule("tabular conversion requires UTF-8 input".into()))?;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(input_delimiter)
        .has_headers(has_header)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut writer = csv::WriterBuilder::new()
        .delimiter(output_delimiter)
        .has_headers(false)
        .from_writer(Vec::new());
    if has_header {
        let headers = reader
            .headers()
            .map_err(|error| DagError::Schedule(format!("cannot read tabular header: {error}")))?;
        writer
            .write_record(headers)
            .map_err(|error| DagError::Schedule(format!("cannot write tabular header: {error}")))?;
    }
    for record in reader.records() {
        let record = record
            .map_err(|error| DagError::Schedule(format!("cannot parse tabular input: {error}")))?;
        writer
            .write_record(&record)
            .map_err(|error| DagError::Schedule(format!("cannot write tabular output: {error}")))?;
    }
    writer
        .into_inner()
        .map_err(|error| DagError::Schedule(format!("cannot finish tabular output: {error}")))
}

fn truncate_bytes(bytes: Vec<u8>, max_bytes: Option<u64>, max_lines: Option<u64>) -> Vec<u8> {
    let mut bytes = if let Some(max_bytes) = max_bytes {
        bytes.into_iter().take(max_bytes as usize).collect()
    } else {
        bytes
    };
    if let Some(max_lines) = max_lines {
        if max_lines == 0 {
            bytes.clear();
            return bytes;
        }
        let mut count = 0_u64;
        let mut end;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                count += 1;
                end = index + 1;
                if count == max_lines {
                    bytes.truncate(end);
                    return bytes;
                }
            }
        }
        if count < max_lines {
            // Keep a final partial line.
            return bytes;
        }
    }
    bytes
}

impl FileTransformNode {
    pub fn new(spec: FileTransformSpec) -> Self {
        Self {
            ports: port_layout(),
            src: spec.src.clone(),
            dst: spec.dst.clone(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for FileTransformNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            src: self.src.clone(),
            dst: self.dst.clone(),
            spec: self.spec.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        FILE_TRANSFORM_KIND
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
            .or_else(|| self.src.clone())
            .ok_or_else(|| {
                DagError::Schedule("file_transform requires an upstream file or src".into())
            })?;
        let source = normalize_and_source(ctx, &source);
        let destination = normalize_and_source(ctx, &self.dst);
        let input = read_bytes(ctx, &source).await?;
        let output = match self.spec.op {
            FileTransformOp::Gunzip => decompress_if_gzip(&source, input)?,
            FileTransformOp::Gzip => compress_for_destination(&destination, input)?,
            FileTransformOp::Convert => {
                let plain = decompress_if_gzip(&source, input)?;
                let input_delimiter = match &self.spec.delimiter {
                    Some(value) => parse_delimiter(value).map_err(DagError::Schedule)?,
                    None => default_delimiter(&source, self.spec.format_in.as_deref()),
                };
                let output_delimiter = match &self.spec.delimiter {
                    Some(_) => input_delimiter,
                    None => default_delimiter(&destination, self.spec.format_out.as_deref()),
                };
                let converted = convert_delimited(
                    plain,
                    input_delimiter,
                    output_delimiter,
                    self.spec.has_header.unwrap_or(true),
                )?;
                if is_gzip(&destination) {
                    compress_for_destination(&destination, converted)?
                } else {
                    converted
                }
            }
            FileTransformOp::Truncate => {
                truncate_bytes(input, self.spec.max_bytes, self.spec.max_lines)
            }
            FileTransformOp::Transcode => {
                let plain = decompress_if_gzip(&source, input)?;
                let text = decode(&self.spec.encoding_in, &plain)?;
                let encoded = encode(&self.spec.encoding_out, &text)?;
                if is_gzip(&destination) {
                    compress_for_destination(&destination, encoded)?
                } else {
                    encoded
                }
            }
        };
        write_bytes(ctx, &destination, output).await?;
        let format = self
            .spec
            .format_out
            .clone()
            .or_else(|| destination.rsplit('.').next().map(str::to_string));
        let file = FileRef::local(&destination, format)
            .unwrap_or_else(|_| FileRef::new(destination.clone(), None));
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

pub struct FileTransformNodeFactory;

impl NodeFactory for FileTransformNodeFactory {
    fn kind(&self) -> &'static str {
        FILE_TRANSFORM_KIND
    }

    fn desc(&self) -> &'static str {
        "Transforms a local or VFS text file without entering a container."
    }

    fn doc(&self) -> &'static str {
        "Operations are gunzip, gzip, convert, truncate, and transcode. \
        Tabular conversion uses an RFC 4180 parser and can change CSV/TSV \
        delimiters. `.gz` emits ordinary gzip; `.bgz` emits blocked BGZF with \
        an EOF marker. The optional File input takes precedence over `src`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FileTransformSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: FileTransformSpec = serde_json::from_value(spec)?;
        if spec.dst.trim().is_empty() {
            return Err("dst cannot be empty".into());
        }
        if spec.op == FileTransformOp::Truncate
            && spec.max_bytes.is_none()
            && spec.max_lines.is_none()
        {
            return Err("truncate requires max_bytes or max_lines".into());
        }
        Ok(Box::new(FileTransformNode::new(spec)))
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

    #[tokio::test]
    async fn converts_csv_to_tsv_with_quoting_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.csv");
        let dst = dir.path().join("out.tsv");
        std::fs::write(&src, "id,name\n1,\"a, b\"\n").unwrap();
        let spec = serde_json::from_value::<FileTransformSpec>(serde_json::json!({
            "op": "convert",
            "src": src.to_string_lossy(),
            "dst": dst.to_string_lossy(),
            "has_header": true
        }))
        .unwrap();
        let mut node = FileTransformNode::new(spec);
        node.execute(
            &ctx(),
            &[],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "id\tname\n1\ta, b\n"
        );
    }

    #[tokio::test]
    async fn round_trips_gzip() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.txt");
        let compressed = dir.path().join("plain.txt.bgz");
        let restored = dir.path().join("restored.txt");
        std::fs::write(&plain, "hello\nsingle-cell\n").unwrap();

        for (op, src, dst) in [
            (
                FileTransformOp::Gzip,
                plain.to_string_lossy().into_owned(),
                compressed.to_string_lossy().into_owned(),
            ),
            (
                FileTransformOp::Gunzip,
                compressed.to_string_lossy().into_owned(),
                restored.to_string_lossy().into_owned(),
            ),
        ] {
            let spec = FileTransformSpec {
                op,
                src: Some(src),
                dst,
                format_in: None,
                format_out: None,
                delimiter: None,
                encoding_in: "utf-8".into(),
                encoding_out: "utf-8".into(),
                has_header: None,
                max_bytes: None,
                max_lines: None,
            };
            let mut node = FileTransformNode::new(spec);
            node.execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(restored).unwrap(),
            "hello\nsingle-cell\n"
        );
        let compressed_bytes = std::fs::read(&compressed).unwrap();
        assert!(compressed_bytes.windows(2).any(|window| window == b"BC"));
    }
}
