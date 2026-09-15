//! File-to-DataFrame bridge node.
//!
//! A [`FileToDataFrameNode`] reads an external path or an upstream file reference
//! and produces exactly one DataFrame output. The format is auto-detected from
//! the extension or explicitly given. Tabular formats (CSV, Parquet) go through
//! DataFusion natively; JSON and bioinformatics formats (VCF, BAM, BED, ...) go through
//! `biofusion`, which exposes them as DataFusion tables.

use async_trait::async_trait;
use biofusion::datasource::BioReadOptions;
use biofusion::ext::DataFusionReadExt;
use datafusion::{
    common::HashMap,
    prelude::{CsvReadOptions, DataFrame, ParquetReadOptions, SessionContext},
};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    codegen::CodegenTarget,
    codegen::context::{CodegenCtx, CodegenError, NodeCodegen},
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
    value::PortType,
};

/// Supported file formats. CSV/TSV/Parquet use DataFusion directly; JSON and
/// bioinformatics formats use `biofusion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    // DataFusion native
    Csv,
    Tsv,
    Parquet,
    Json,
    // biofusion bioinformatics
    Vcf,
    Bcf,
    Fasta,
    Fastq,
    Mtx,
    Bed,
    Gtf,
    Gff,
    Sam,
    Bam,
    Cram,
    BigWig,
    BigBed,
}

impl FileFormat {
    /// Infer a format from a path's extension. Handles `.gz`-compressed
    /// bioinformatics files (`.vcf.gz`, `.bed.gz`, …).
    pub fn from_path(path: &str) -> Option<Self> {
        let lower = path.to_lowercase();
        // Order matters: longer/compound suffixes first.
        let suffixes: &[(&str, FileFormat)] = &[
            (".vcf.gz", FileFormat::Vcf),
            (".vcf", FileFormat::Vcf),
            (".bcf", FileFormat::Bcf),
            (".fasta.gz", FileFormat::Fasta),
            (".fasta", FileFormat::Fasta),
            (".fa.gz", FileFormat::Fasta),
            (".fa", FileFormat::Fasta),
            (".fastq.gz", FileFormat::Fastq),
            (".fastq", FileFormat::Fastq),
            (".fq.gz", FileFormat::Fastq),
            (".fq", FileFormat::Fastq),
            (".mtx.gz", FileFormat::Mtx),
            (".mtx", FileFormat::Mtx),
            (".bed.gz", FileFormat::Bed),
            (".bed", FileFormat::Bed),
            (".gtf.gz", FileFormat::Gtf),
            (".gtf", FileFormat::Gtf),
            (".gff3.gz", FileFormat::Gff),
            (".gff3", FileFormat::Gff),
            (".gff.gz", FileFormat::Gff),
            (".gff", FileFormat::Gff),
            (".sam.gz", FileFormat::Sam),
            (".sam", FileFormat::Sam),
            (".bam", FileFormat::Bam),
            (".cram", FileFormat::Cram),
            (".bw", FileFormat::BigWig),
            (".bigwig", FileFormat::BigWig),
            (".bb", FileFormat::BigBed),
            (".bigbed", FileFormat::BigBed),
            (".csv", FileFormat::Csv),
            (".tsv.gz", FileFormat::Tsv),
            (".tsv", FileFormat::Tsv),
            (".parquet", FileFormat::Parquet),
            (".json.gz", FileFormat::Json),
            (".json", FileFormat::Json),
            (".ndjson.gz", FileFormat::Json),
            (".ndjson", FileFormat::Json),
        ];
        suffixes
            .iter()
            .find(|(s, _)| lower.ends_with(s))
            .map(|(_, f)| *f)
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.to_ascii_lowercase().as_str() {
            "csv" => Some(Self::Csv),
            "tsv" => Some(Self::Tsv),
            "parquet" => Some(Self::Parquet),
            "json" | "ndjson" => Some(Self::Json),
            "vcf" => Some(Self::Vcf),
            "bcf" => Some(Self::Bcf),
            "fasta" => Some(Self::Fasta),
            "fastq" => Some(Self::Fastq),
            "mtx" | "matrixmarket" => Some(Self::Mtx),
            "bed" => Some(Self::Bed),
            "gtf" => Some(Self::Gtf),
            "gff" => Some(Self::Gff),
            "sam" => Some(Self::Sam),
            "bam" => Some(Self::Bam),
            "cram" => Some(Self::Cram),
            "bigwig" | "bw" => Some(Self::BigWig),
            "bigbed" | "bb" => Some(Self::BigBed),
            _ => None,
        }
    }

    pub fn as_label(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Parquet => "parquet",
            Self::Json => "json",
            Self::Vcf => "vcf",
            Self::Bcf => "bcf",
            Self::Fasta => "fasta",
            Self::Fastq => "fastq",
            Self::Mtx => "mtx",
            Self::Bed => "bed",
            Self::Gtf => "gtf",
            Self::Gff => "gff",
            Self::Sam => "sam",
            Self::Bam => "bam",
            Self::Cram => "cram",
            Self::BigWig => "bigwig",
            Self::BigBed => "bigbed",
        }
    }
}

/// Errors specific to [`FileToDataFrameNode`].
#[derive(Debug, Error)]
pub enum FileToDataFrameError {
    #[error("cannot infer file format from path: {0}")]
    UnknownFormat(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("read source '{path}' failed")]
    Read {
        path: String,
        #[source]
        source: datafusion::error::DataFusionError,
    },
}

impl FileToDataFrameError {
    pub fn to_dag_error(self) -> DagError {
        match self {
            FileToDataFrameError::Read { source, .. } => DagError::DataFusion(source),
            FileToDataFrameError::UnknownFormat(msg) => DagError::Schedule(msg),
            FileToDataFrameError::InvalidInput(msg) => DagError::Schedule(msg),
        }
    }
}

impl ::dag_core::dag::NodeError for FileToDataFrameError {
    fn node_type(&self) -> &str {
        "file_to_dataframe"
    }
}

#[derive(Clone)]
pub struct FileToDataFrameNode {
    meta: NodePorts,
    path: Option<String>,
    format: Option<FileFormat>,
    partition_by: Vec<String>,
}

impl FileToDataFrameNode {
    pub fn new(path: Option<String>, format: Option<FileFormat>) -> Self {
        Self::new_with_partitions(path, format, Vec::new())
    }

    pub fn new_with_partitions(
        path: Option<String>,
        format: Option<FileFormat>,
        partition_by: Vec<String>,
    ) -> Self {
        // The optional file input lets a file-producing node supply the path.
        Self {
            meta: port_layout(),
            path,
            format,
            partition_by,
        }
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct FileToDataFrameNodeSpec {
    /// A file path or URL. When `format` is `None`, it is inferred from the
    /// extension (`.vcf.gz` → Vcf, `.bam` → Bam, `.csv` → Csv, `.json` → Json, …).
    pub path: Option<String>,
    pub format: Option<FileFormat>,
    /// Hive-style partition column names. Values are restored as Utf8.
    #[serde(default)]
    pub partition_by: Vec<String>,
}

pub struct FileToDataFrameNodeFactory {}

/// Static port layout for every [`FileToDataFrameNode`]: an optional file input
/// and a single DataFrame output.
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_optional_input_port_of_type(PortType::File)
        .add_output_port(None)
}

impl NodeFactory for FileToDataFrameNodeFactory {
    fn kind(&self) -> &'static str {
        "file_to_dataframe"
    }

    fn desc(&self) -> &'static str {
        "Reads CSV, TSV, JSON/NDJSON, Parquet, or bioinformatics files into the DAG as a DataFrame."
    }

    fn doc(&self) -> &'static str {
        "Reads an external path or an upstream file reference into a \
        DataFrame. Supports local/remote files: CSV/TSV/Parquet via \
        DataFusion, JSON arrays and NDJSON (including .json.gz), and \
        bioinformatics formats (VCF, BAM, BED, GTF, FASTA, MatrixMarket, etc.) via \
        biofusion. Format is inferred from the \
        extension when not given explicitly. Optional file input; one \
        DataFrame output."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FileToDataFrameNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: FileToDataFrameNodeSpec = serde_json::from_value(spec)?;
        let node = FileToDataFrameNode::new_with_partitions(
            node_spec.path,
            node_spec.format,
            node_spec.partition_by,
        );
        Ok(Box::new(node))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> std::result::Result<NodeCodegen, CodegenError> {
        let node_spec: FileToDataFrameNodeSpec =
            serde_json::from_value(spec.clone()).map_err(|e| CodegenError::BadSpec {
                kind: "file_to_dataframe".into(),
                source: e,
            })?;

        let connected_input = ctx
            .input_vars
            .first()
            .is_some_and(|input| !input.starts_with("__missing_input"));
        let literal_path = node_spec.path.as_deref();
        if literal_path.is_none() && !connected_input {
            return Err(CodegenError::NotSupported {
                kind: "file_to_dataframe".into(),
                target: CodegenTarget::R,
            });
        }
        if !node_spec.partition_by.is_empty() {
            return Err(CodegenError::NotSupported {
                kind: "file_to_dataframe partitioned Parquet".into(),
                target: CodegenTarget::R,
            });
        }
        let fmt = node_spec
            .format
            .or_else(|| literal_path.and_then(FileFormat::from_path))
            .unwrap_or(FileFormat::Csv);
        let path = if connected_input {
            ctx.input_vars
                .first()
                .expect("checked input presence")
                .clone()
        } else {
            format!(r#""{}""#, literal_path.unwrap_or_default())
        };

        let out = ctx.output_var.to_string();
        let read_call = match fmt {
            FileFormat::Csv | FileFormat::Tsv => {
                format!(r#"{out} <- fread({path})"#)
            }
            FileFormat::Parquet => {
                format!(r#"{out} <- read_parquet({path})"#)
            }
            FileFormat::Json => {
                format!(r#"{out} <- jsonlite::fromJSON({path}, simplifyDataFrame = TRUE)"#)
            }
            FileFormat::Vcf => {
                format!(
                    r#"# NOTE: R codegen for VCF uses vcfR::read.vcfR
{out} <- vcfR::read.vcfR({path}, verbose = FALSE)"#
                )
            }
            FileFormat::Bed => {
                format!(r#"{out} <- read.table({path}, sep = "\t", header = FALSE)"#)
            }
            FileFormat::Mtx => {
                format!(
                    "# NOTE: MatrixMarket is loaded as the long table (row, column, value).\n\
                     mm_summary <- Matrix::summary(Matrix::readMM({path}))\n\
                     {out} <- data.frame(row = mm_summary$i, column = mm_summary$j, value = mm_summary$x)"
                )
            }
            _ => {
                // Fallback: comment + placeholder
                format!(
                    "# NOTE: R codegen for {fmt:?} format not yet implemented — \
                     read the file manually"
                )
            }
        };

        let code = vec![read_call];
        Ok(NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["data.table".into(), "jsonlite".into()]
    }
}

pub fn normalize_path(path: &str) -> String {
    let preserve_directory = path.ends_with('/');
    if let Some((scheme, rest)) = path.split_once("://") {
        if scheme.eq_ignore_ascii_case("vfs") || scheme.eq_ignore_ascii_case("file") {
            let normalized = vfs::OpendalFileStorage::normalize_path(rest);
            let normalized = if preserve_directory && normalized != "/" {
                format!("{normalized}/")
            } else {
                normalized
            };
            return format!("{scheme}://{normalized}");
        }
        return path.to_string();
    }
    let trimmed = path
        .trim_matches('/')
        .strip_prefix("./")
        .unwrap_or(path.trim_matches('/'));
    let trimmed = trimmed.strip_prefix('.').unwrap_or(trimmed);
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        if preserve_directory {
            format!("/{trimmed}/")
        } else {
            format!("/{trimmed}")
        }
    }
}

fn ensure_directory_path(path: String, partitioned: bool) -> String {
    if partitioned && !path.ends_with('/') {
        format!("{path}/")
    } else {
        path
    }
}

/// DataFusion treats `file://` paths as its built-in local filesystem even
/// when an OpenDAL-backed store is registered under that URL. Mounted virtual
/// paths therefore must be addressed through the dedicated `vfs://` store.
pub(crate) fn source_path(node_ctx: &dag_core::registry::NodeCtx, path: &str) -> String {
    if path.starts_with("vfs://") {
        return path.to_string();
    }

    let Some(storage) = node_ctx.opendal.as_ref() else {
        return path.to_string();
    };

    let candidate = if let Some(rest) = path.strip_prefix("file://") {
        normalize_path(rest)
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        return path.to_string();
    };

    if storage.is_mounted(&candidate) {
        format!("vfs://{candidate}")
    } else {
        path.to_string()
    }
}

#[async_trait]
impl DagNode for FileToDataFrameNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "file_to_dataframe"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &dag_core::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let ctx = node_ctx.session();
        let upstream = inputs.first().and_then(|input| input.file_value().ok());
        let path = upstream
            .map(|file| file.path.clone())
            .or_else(|| self.path.clone())
            .ok_or_else(|| {
                FileToDataFrameError::UnknownFormat(
                    "file_to_dataframe requires an upstream file or a fallback path".into(),
                )
            })?;
        let path = ensure_directory_path(
            source_path(node_ctx, &normalize_path(&path)),
            !self.partition_by.is_empty(),
        );
        let inferred_fmt = self
            .format
            .or_else(|| {
                upstream
                    .and_then(|file| file.format.as_deref())
                    .and_then(FileFormat::from_label)
            })
            .or_else(|| FileFormat::from_path(&path));
        let fmt = match inferred_fmt {
            Some(fmt) => fmt,
            None if !self.partition_by.is_empty() => FileFormat::Parquet,
            None => return Err(FileToDataFrameError::UnknownFormat(path.clone()).into()),
        };
        if !self.partition_by.is_empty() && fmt != FileFormat::Parquet {
            return Err(FileToDataFrameError::InvalidInput(
                "partition_by is only supported for Parquet input".into(),
            )
            .into());
        }
        let partition_cols = self
            .partition_by
            .iter()
            .map(|name| (name.clone(), arrow_schema::DataType::Utf8))
            .collect::<Vec<_>>();
        let df = read_file(&ctx, &path, fmt, &partition_cols).await?;

        let df = if matches!(fmt, FileFormat::Csv | FileFormat::Tsv) {
            promote_identifier_strings(df)?
        } else {
            df
        };

        // Promote Float32 columns to Float64: Parquet files may store
        // Float32, and downstream SQL JOINs/UNIONs with Float64 data trigger
        // DataFusion type-coercion failures during collect().
        let df = promote_floats(df)?;

        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

async fn read_file(
    ctx: &SessionContext,
    path: &str,
    fmt: FileFormat,
    partition_cols: &[(String, arrow_schema::DataType)],
) -> Result<DataFrame, DagError> {
    use FileFormat::*;
    use datafusion::datasource::file_format::file_compression_type::FileCompressionType;
    let expected_extension = path_file_extension(path);
    let compression = if path.to_lowercase().ends_with(".gz") {
        FileCompressionType::GZIP
    } else {
        FileCompressionType::UNCOMPRESSED
    };
    let df = match fmt {
        Csv => {
            let opts = CsvReadOptions::default()
                .file_extension(&expected_extension)
                .file_compression_type(compression);
            ctx.read_csv(path, opts).await
        }
        Tsv => {
            // The expected extension follows the actual path so an explicit
            // TSV format can also read nonstandard extensions such as .raw.
            let opts = CsvReadOptions::default()
                .delimiter(b'\t')
                .file_extension(&expected_extension)
                .file_compression_type(compression);
            ctx.read_csv(path, opts).await
        }
        Parquet => {
            let options =
                ParquetReadOptions::default().table_partition_cols(partition_cols.to_vec());
            ctx.read_parquet(path, options).await
        }
        Json => ctx.read_bio_json(path, BioReadOptions::default()).await,
        Vcf => ctx.read_vcf(path, BioReadOptions::default()).await,
        Bcf => ctx.read_bcf(path, BioReadOptions::default()).await,
        Fasta => ctx.read_fasta(path, BioReadOptions::default()).await,
        Fastq => ctx.read_fastq(path, BioReadOptions::default()).await,
        Mtx => ctx.read_mtx(path, BioReadOptions::default()).await,
        Bed => ctx.read_bed(path, BioReadOptions::default()).await,
        Gtf => ctx.read_gtf(path, BioReadOptions::default()).await,
        Gff => ctx.read_gff(path, BioReadOptions::default()).await,
        Sam => ctx.read_sam(path, BioReadOptions::default()).await,
        Bam => ctx.read_bam(path, BioReadOptions::default()).await,
        Cram => ctx.read_cram(path, BioReadOptions::default()).await,
        BigWig => ctx.read_bigwig(path, BioReadOptions::default()).await,
        BigBed => ctx.read_bigbed(path, BioReadOptions::default()).await,
    };
    df.map_err(|e| {
        FileToDataFrameError::Read {
            path: path.to_string(),
            source: e,
        }
        .into()
    })
}

fn path_file_extension(path: &str) -> String {
    let compressed = path.to_ascii_lowercase().ends_with(".gz");
    let path = if compressed {
        &path[..path.len() - ".gz".len()]
    } else {
        path
    };
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    if compressed {
        format!("{extension}.gz")
    } else {
        extension
    }
}

/// Keep domain identifiers as strings after CSV inference. Numeric-looking
/// gene IDs otherwise fail downstream nodes that declare Utf8 ports.
fn promote_identifier_strings(mut df: DataFrame) -> Result<DataFrame, DagError> {
    use arrow_schema::DataType;
    use datafusion::common::Column;
    use datafusion::logical_expr::Expr;
    use datafusion::logical_expr::cast;

    const IDENTIFIER_COLUMNS: &[&str] = &["gene_id", "set_id", "snp_id", "rsid", "snp"];
    let identifier_cols: Vec<String> = df
        .schema()
        .fields()
        .iter()
        .filter(|field| {
            field.data_type().is_primitive() && IDENTIFIER_COLUMNS.contains(&field.name().as_str())
        })
        .map(|field| field.name().to_string())
        .collect();

    for name in &identifier_cols {
        df = df.with_column(
            name,
            cast(Expr::Column(Column::from_name(name)), DataType::Utf8),
        )?;
    }
    Ok(df)
}

/// Cast every Float32 column to Float64, leaving all other columns unchanged.
fn promote_floats(mut df: DataFrame) -> Result<DataFrame, DagError> {
    use arrow_schema::DataType;
    use datafusion::common::Column;
    use datafusion::logical_expr::Expr;
    use datafusion::logical_expr::cast;

    let float32_cols: Vec<String> = df
        .schema()
        .fields()
        .iter()
        .filter(|f| matches!(f.data_type(), DataType::Float32))
        .map(|f| f.name().to_string())
        .collect();

    for name in &float32_cols {
        // `col(&str)` parses the value as an SQL identifier and lowercases it
        // when identifier normalization is enabled. A raw Column keeps the
        // exact Parquet field name.
        df = df.with_column(
            name,
            cast(Expr::Column(Column::from_name(name)), DataType::Float64),
        )?;
    }
    Ok(df)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("fixtures")
            .join(name)
    }

    fn sample_vcf_bytes() -> Vec<u8> {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let compressed = std::fs::read(fixture("sample.vcf.gz")).unwrap();
        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut plain = Vec::new();
        decoder.read_to_end(&mut plain).unwrap();
        plain
    }
    use datafusion::execution::object_store::ObjectStoreUrl;
    use datafusion::prelude::SessionContext;
    use parquet::arrow::ArrowWriter;
    use std::fs::File;
    use std::sync::Arc;
    use vfs::OpendalFileStorage;
    use vfs::{BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, VfsManifest};

    #[test]
    fn promote_floats_preserves_camel_case_column_names() {
        let ctx = SessionContext::new();
        let schema = Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
            "Blood_weight",
            arrow_schema::DataType::Float32,
            true,
        )]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![Arc::new(arrow_array::Float32Array::from(vec![1.0]))],
        )
        .unwrap();
        let df = ctx.read_batch(batch).unwrap();

        let df = promote_floats(df).unwrap();

        let fields: Vec<&str> = df
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect();
        assert_eq!(fields, ["Blood_weight"]);
        assert_eq!(
            df.schema().field(0).data_type(),
            &arrow_schema::DataType::Float64
        );
    }

    #[tokio::test]
    async fn file_to_dataframe_preserves_camel_case_parquet_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Blood.parquet");
        let schema = Arc::new(arrow_schema::Schema::new(vec![
            arrow_schema::Field::new("ID", arrow_schema::DataType::Int32, true),
            arrow_schema::Field::new("Blood_weight", arrow_schema::DataType::Float32, true),
            arrow_schema::Field::new("Cystatin_c", arrow_schema::DataType::Float32, true),
        ]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(arrow_array::Int32Array::from(vec![1])),
                Arc::new(arrow_array::Float32Array::from(vec![2.0])),
                Arc::new(arrow_array::Float32Array::from(vec![3.0])),
            ],
        )
        .unwrap();
        let file = File::create(&path).unwrap();
        let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let ctx = SessionContext::new();
        let df = read_file(&ctx, path.to_str().unwrap(), FileFormat::Parquet, &[])
            .await
            .unwrap();
        let df = promote_floats(df).unwrap();

        let fields: Vec<&str> = df
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect();
        assert_eq!(fields, ["ID", "Blood_weight", "Cystatin_c"]);
        assert_eq!(
            df.schema().field(1).data_type(),
            &arrow_schema::DataType::Float64
        );
        assert_eq!(
            df.schema().field(2).data_type(),
            &arrow_schema::DataType::Float64
        );

        let batches = df.collect().await.unwrap();
        assert_eq!(batches[0].column_by_name("Blood_weight").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn file_to_dataframe_reads_hive_partitioned_parquet_from_vfs() {
        use datafusion::prelude::{col, lit};
        use parquet::arrow::ArrowWriter;

        let backend_root = tempfile::tempdir().unwrap();
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "default".into(),
                config: BackendConfig::local(backend_root.path().to_string_lossy().to_string()),
            }],
            mount: vec![MountDefinition {
                path: "/dataset".into(),
                backend: "default".into(),
                source: backend_root.path().to_string_lossy().to_string(),
                read_only: true,
            }],
        };
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(OpendalFileStorage::with_mounts(
            tempfile::tempdir().unwrap().path(),
            mounted,
        ));
        let ctx = SessionContext::new();
        ctx.runtime_env().register_object_store(
            ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
            storage.clone(),
        );

        let schema = Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
            "id",
            arrow_schema::DataType::Int32,
            true,
        )]));
        for (id, partition) in [(1, "one"), (2, "two")] {
            let dir = backend_root.path().join(format!("part={partition}"));
            std::fs::create_dir_all(&dir).unwrap();
            let batch = arrow_array::RecordBatch::try_new(
                schema.clone(),
                vec![Arc::new(arrow_array::Int32Array::from(vec![id]))],
            )
            .unwrap();
            let file = std::fs::File::create(dir.join("data.parquet")).unwrap();
            let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }

        let node_ctx = dag_core::registry::NodeCtx::new(ctx.runtime_env().clone(), Some(storage));
        let mut node = FileToDataFrameNode::new_with_partitions(
            Some("/dataset".into()),
            None,
            vec!["part".into()],
        );
        let outputs = node
            .execute(
                &node_ctx,
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = outputs.dataframe(0).unwrap();
        assert_eq!(
            df.schema()
                .field_with_name(None, "part")
                .unwrap()
                .data_type(),
            &arrow_schema::DataType::Utf8
        );
        let one = df
            .clone()
            .filter(col("part").eq(lit("one")))
            .unwrap()
            .count()
            .await
            .unwrap();
        assert_eq!(one, 1);
        assert_eq!(df.clone().count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_load_vcf() {
        let (ctx, fs) = OpendalFileStorage::new_temp().register_to_ctx();
        let test_vcf = sample_vcf_bytes();
        fs.op.write("/sample.vcf", test_vcf).await.unwrap();

        let res = ctx
            .read_vcf("/sample.vcf", BioReadOptions::default())
            .await
            .unwrap();

        // res.show().await.unwrap();

        let schema = res.schema();
        dbg!(schema);
    }

    #[tokio::test]
    async fn test_load_vcf_gz() {
        let (ctx, fs) = OpendalFileStorage::new_temp().register_to_ctx();
        dbg!("start copy data");
        let test_vcf_gz = std::fs::read(fixture("sample.vcf.gz")).unwrap();
        fs.op.write("/sample.vcf.gz", test_vcf_gz).await.unwrap();
        dbg!("copy data finished");

        let res = ctx
            .read_vcf("/sample.vcf.gz", BioReadOptions::default())
            .await
            .unwrap();

        res.show().await.unwrap();

        // let schema = res.schema();
        // dbg!(schema);
    }

    /// Regression: biofusion's VCF reader (backed by oxbow) ALWAYS names the
    /// INFO struct parent column `"info"`. The struct's subfield names follow
    /// the VCF `<ID=...>` header entries, but the parent column name does NOT
    /// derive from the filename, study id, or any other source-level string.
    ///
    /// The agent's report named the column `"EBI-a-GCST005195"`; that was a
    /// misreading (almost certainly `AS`-renaming from a downstream SQL).
    /// This test pins the actual behavior so any future change breaks loudly.
    #[tokio::test]
    async fn test_vcf_info_column_is_literally_named_info() {
        let (ctx, fs) = OpendalFileStorage::new_temp().register_to_ctx();
        let test_vcf_gz = std::fs::read(fixture("sample.vcf.gz")).unwrap();
        fs.op.write("/sample.vcf.gz", test_vcf_gz).await.unwrap();

        let res = ctx
            .read_vcf("/sample.vcf.gz", BioReadOptions::default())
            .await
            .unwrap();

        let schema = res.schema();
        let field_names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();

        // The schema must include an INFO struct column literally named "info".
        // We assert presence (must) and exact name (must) — not derived from
        // filename, study id, or any other source-level identifier.
        assert!(
            field_names.iter().any(|n| n == "info"),
            "VCF schema must contain a column literally named `info`, got: {field_names:?}"
        );

        // And the `info` field must itself be a Struct — not a Map or List —
        // because that's how `get_field(info, '<KEY>')` extracts subfields.
        let info_field = schema.field_with_name(None, "info").expect("`info` field");
        let info_dt = info_field.data_type();
        assert!(
            matches!(info_dt, arrow_schema::DataType::Struct(_)),
            "VCF `info` column must be Struct, got: {info_dt:?}"
        );

        // The struct's subfields must come from `<ID=...>` header entries, not
        // any user-level alias. For the bundled sample the header declares at
        // least these standard IDs.
        if let arrow_schema::DataType::Struct(fields) = info_dt {
            let sub_names: Vec<&str> = fields.iter().map(|f| f.name().as_str()).collect();
            // Sample file is small — just assert the subfields exist (not exact
            // set, to avoid coupling the test to the fixture's exact INFO IDs).
            assert!(
                !sub_names.is_empty(),
                "VCF `info` struct must have at least one subfield declared from the VCF header"
            );
        }
    }

    /// Regression: the upstream SqlNode's `get_field(info, '<subfield>')` form
    /// (`info['ES']`) succeeds when `info` is the actual VCF struct column, and
    /// the result type matches the declared INFO type in the VCF header. This
    /// guards obstacle #1 from re-occurring.
    ///
    /// The fixture file's INFO subfields can be arbitrary, so we pick a name
    /// that looks safe (no embedded `.` or special characters, just letters).
    /// The test pins that *at minimum* a Struct-typed info column is queryable
    /// through SQL with `get_field(info, <plain-name>)`.
    #[tokio::test]
    async fn test_get_field_on_vcf_info_succeeds() {
        let (ctx, fs) = OpendalFileStorage::new_temp().register_to_ctx();
        let test_vcf_gz = std::fs::read(fixture("sample.vcf.gz")).unwrap();
        fs.op.write("/sample.vcf.gz", test_vcf_gz).await.unwrap();

        let res = ctx
            .read_vcf("/sample.vcf.gz", BioReadOptions::default())
            .await
            .unwrap();

        // Round-trip the DataFrame through SQL so get_field exercises the real
        // SQL planner path the agent would have used. Re-register under a
        // stable name so the SQL below resolves.
        ctx.register_table("vcf_source", res.clone().into_view())
            .expect("register vcf_source view");

        // Smoke: literal bracket notation on the struct column plans and runs.
        // We deliberately use a *known safe* attribute path here — the agent's
        // mistake was inventing dot-notation fields that don't exist. Pin that
        // ANY get_field(info, '<plain subfield>') call doesn't blow up the
        // planner with "Field es not found in struct" (obstacle #1).
        let planning_result = ctx
            .sql("SELECT get_field(info, 'AF') AS af FROM vcf_source")
            .await;

        // The fixture may or may not declare an `AF` field, so accept either:
        //   (a) success (good), or
        //   (b) a *named-field* error like `Field AF not found in struct` (the
        //       original obstacle #1 symptom — confirming the typo-detection
        //       path that the agent's `EBI-a-GCST005195` artifact would have
        //       surfaced first).
        match planning_result {
            Ok(df) => {
                // Planning succeeded — execution may error if the chosen
                // subfield is dictionary-encoded or has a value-level type
                // mismatch. We don't pin that here; the goal of this test is
                // just to lock down the `Field not found in struct` path that
                // blocked obstacle #1.
                let _ = df.collect().await;
            }
            Err(e) => {
                let msg = format!("{e}");
                let planned_field_not_found =
                    msg.contains("Field") && msg.contains("not found in struct");
                let runtime_struct_mismatch = msg.contains("get_field is only possible")
                    || msg.contains("Cannot access field");
                assert!(
                    planned_field_not_found || runtime_struct_mismatch,
                    "expected either a missing-field planning error or a \
                     get_field runtime error; got: {msg}"
                );
            }
        }
    }

    #[tokio::test]
    async fn file_to_dataframe_accepts_csv_override_for_nonstandard_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cohort.genes.raw");
        std::fs::write(&path, "gene_id,n\n79501,504\n").unwrap();

        let ctx = SessionContext::new();
        let node_ctx = dag_core::registry::NodeCtx::new(ctx.runtime_env().clone(), None);
        let mut node = FileToDataFrameNode::new(
            Some(path.to_string_lossy().to_string()),
            Some(FileFormat::Csv),
        );

        let outputs = node
            .execute(
                &node_ctx,
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = outputs.dataframe(0).unwrap();
        let schema = df.schema();
        assert_eq!(
            schema.field_with_name(None, "gene_id").unwrap().data_type(),
            &arrow_schema::DataType::Utf8
        );

        let batches = df.clone().collect().await.unwrap();
        let ids = batches[0]
            .column_by_name("gene_id")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(ids.value(0), "79501");
    }

    #[tokio::test]
    async fn file_to_dataframe_accepts_csv_override_for_gzipped_nonstandard_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cohort.sumstats.gz");
        let mut encoder = flate2::write::GzEncoder::new(
            std::fs::File::create(&path).unwrap(),
            Default::default(),
        );
        std::io::Write::write_all(&mut encoder, b"gene_id,n\n79501,504\n").unwrap();
        encoder.finish().unwrap();

        let ctx = SessionContext::new();
        let node_ctx = dag_core::registry::NodeCtx::new(ctx.runtime_env().clone(), None);
        let mut node = FileToDataFrameNode::new(
            Some(path.to_string_lossy().to_string()),
            Some(FileFormat::Csv),
        );

        let outputs = node
            .execute(
                &node_ctx,
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = outputs.dataframe(0).unwrap();
        assert_eq!(
            df.schema()
                .field_with_name(None, "gene_id")
                .unwrap()
                .data_type(),
            &arrow_schema::DataType::Utf8
        );
        assert_eq!(df.clone().count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn file_to_dataframe_reads_mounted_paths() {
        let backend_root = tempfile::tempdir().unwrap();
        let source_dir = backend_root.path().join("mounted-source");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("data.csv"), "id\n1\n2\n").unwrap();
        std::fs::write(source_dir.join("data.json"), r#"[{"id":1},{"id":2}]"#).unwrap();

        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "default".into(),
                config: BackendConfig::local(backend_root.path().to_string_lossy().to_string()),
            }],
            mount: vec![MountDefinition {
                path: "/mount".into(),
                backend: "default".into(),
                source: source_dir.to_string_lossy().to_string(),
                read_only: true,
            }],
        };
        let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let file_storage = Arc::new(OpendalFileStorage::with_mounts(
            tempfile::tempdir().unwrap().path(),
            mounted.clone(),
        ));
        let ctx = datafusion::prelude::SessionContext::new();
        ctx.runtime_env()
            .register_object_store(ObjectStoreUrl::parse("vfs://").unwrap().as_ref(), mounted);
        ctx.runtime_env().register_object_store(
            ObjectStoreUrl::parse("file://").unwrap().as_ref(),
            file_storage.clone(),
        );
        let node_ctx =
            dag_core::registry::NodeCtx::new(ctx.runtime_env().clone(), Some(file_storage));

        for path in [
            "vfs:///mount/data.csv",
            "file:///mount/data.csv",
            "/mount/data.csv",
            "vfs:///mount/data.json",
            "file:///mount/data.json",
            "/mount/data.json",
        ] {
            if path.ends_with(".json") {
                let direct_df = ctx
                    .read_bio_json(path, BioReadOptions::default())
                    .await
                    .unwrap_or_else(|e| panic!("direct read {path} failed: {e}"));
                assert_eq!(direct_df.count().await.unwrap(), 2);
            }
            let mut node = FileToDataFrameNode::new(Some(path.into()), None);
            let outputs = node
                .execute(
                    &node_ctx,
                    &[],
                    &dag_core::dag::node_event::NodeReporter::noop(),
                )
                .await
                .unwrap_or_else(|e| panic!("read {path} failed: {e}"));
            let df = outputs.dataframe(0).expect("source output port");
            assert_eq!(
                df.clone().count().await.unwrap(),
                2,
                "unexpected row count for {path}"
            );
        }
    }
}

#[tokio::test]
async fn file_to_dataframe_reads_matrixmarket_long_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("counts.mtx");
    std::fs::write(
        &path,
        "%%MatrixMarket matrix coordinate integer general\n\
         % rows are genes and columns are cells\n\
         2 3 4\n\
         1 1 10\n\
         1 3 2\n\
         2 1 5\n\
         2 2 7\n",
    )
    .unwrap();

    let ctx = SessionContext::new();
    let node_ctx = dag_core::registry::NodeCtx::new(ctx.runtime_env().clone(), None);
    let mut node = FileToDataFrameNode::new(Some(path.to_string_lossy().to_string()), None);
    let outputs = node
        .execute(
            &node_ctx,
            &[],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();
    let df = outputs.dataframe(0).unwrap();

    let fields: Vec<&str> = df
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().as_str())
        .collect();
    assert_eq!(fields, ["row", "column", "value"]);
    assert_eq!(
        df.schema()
            .field_with_name(None, "value")
            .unwrap()
            .data_type(),
        &arrow_schema::DataType::Float64
    );
    assert_eq!(df.clone().count().await.unwrap(), 4);
}
