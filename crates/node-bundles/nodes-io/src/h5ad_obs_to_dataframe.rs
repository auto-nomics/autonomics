//! H5AD `/obs` to DataFusion DataFrame bridge.
//!
//! This module deliberately does not interpret the expression matrix. It reads
//! AnnData-encoded observation metadata in row batches and exposes a lazily
//! executed DataFusion table.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    ArrayRef, BooleanArray, DictionaryArray, PrimitiveArray, RecordBatch, StringArray,
    builder::{PrimitiveBuilder, StringDictionaryBuilder},
    types::{
        Float32Type, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type, UInt8Type, UInt16Type,
        UInt32Type, UInt64Type,
    },
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::streaming::StreamingTable;
use datafusion::physical_plan::stream::RecordBatchReceiverStreamBuilder;
use datafusion::physical_plan::streaming::PartitionStream;
use futures::StreamExt;
use hdf5::types::{TypeDescriptor, VarLenAscii, VarLenUnicode};
use hdf5::{Dataset, File, Group};
use hdf5_sys::h5::herr_t;
use hdf5_sys::h5i::hid_t;
use hdf5_sys::h5p::{H5Pget_chunk, H5Pmodify_filter};
use hdf5_sys::h5t::H5Tget_size;
use hdf5_sys::h5z::{H5Z_CLASS_T_VERS, H5Z_FLAG_REVERSE, H5Z_class2_t, H5Z_filter_t, H5Zregister};
use lzf_sys::{LZF_VERSION, lzf_compress, lzf_decompress};
use ndarray::s;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use tempfile::NamedTempFile;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
    value::PortType,
};

pub const H5AD_OBS_TO_DATAFRAME_KIND: &str = "h5ad_obs_to_dataframe";
const DEFAULT_BATCH_SIZE: usize = 65_536;
const CHANNEL_CAPACITY: usize = 2;
const LZF_FILTER_NAME: &[u8] = b"autonomics-lzf\0";
const LZF_FILTER_ID: H5Z_filter_t = 32_000;
const LZF_FILTER_VERSION: u32 = 4;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct H5adObsToDataFrameSpec {
    /// Local or `vfs://` H5AD path. An upstream File input takes precedence.
    pub path: Option<String>,
    #[serde(default)]
    pub include_obsm: Vec<String>,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Validate that all obs IDs are unique. Upstream H5AD nodes already
    /// enforce this contract, so it can be disabled for very large inputs.
    #[serde(default = "default_true")]
    pub validate_unique_ids: bool,
}

fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumericType {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
}

impl NumericType {
    fn from_descriptor(descriptor: &TypeDescriptor) -> Option<Self> {
        match descriptor {
            TypeDescriptor::Integer(size) => match *size {
                hdf5::types::IntSize::U1 => Some(Self::I8),
                hdf5::types::IntSize::U2 => Some(Self::I16),
                hdf5::types::IntSize::U4 => Some(Self::I32),
                hdf5::types::IntSize::U8 => Some(Self::I64),
            },
            TypeDescriptor::Unsigned(size) => match *size {
                hdf5::types::IntSize::U1 => Some(Self::U8),
                hdf5::types::IntSize::U2 => Some(Self::U16),
                hdf5::types::IntSize::U4 => Some(Self::U32),
                hdf5::types::IntSize::U8 => Some(Self::U64),
            },
            TypeDescriptor::Float(size) => match *size {
                hdf5::types::FloatSize::U4 => Some(Self::F32),
                hdf5::types::FloatSize::U8 => Some(Self::F64),
            },
            _ => None,
        }
    }

    fn arrow_type(self) -> DataType {
        match self {
            Self::I8 => DataType::Int8,
            Self::I16 => DataType::Int16,
            Self::I32 => DataType::Int32,
            Self::I64 => DataType::Int64,
            Self::U8 => DataType::UInt8,
            Self::U16 => DataType::UInt16,
            Self::U32 => DataType::UInt32,
            Self::U64 => DataType::UInt64,
            Self::F32 => DataType::Float32,
            Self::F64 => DataType::Float64,
        }
    }
}

#[derive(Debug, Clone)]
enum ObsDecoder {
    Numeric(NumericType),
    Boolean,
    String,
    NullableNumeric(NumericType),
    NullableBoolean,
    NullableString,
    Categorical { categories: Vec<String> },
}

#[derive(Debug)]
enum ColumnSource {
    Index,
    Obs {
        name: String,
        decoder: ObsDecoder,
    },
    Obsm {
        key: String,
        dimension: usize,
        dtype: NumericType,
    },
}

#[derive(Debug)]
struct H5adObsColumn {
    name: String,
    source: ColumnSource,
}

#[derive(Debug)]
struct H5adObsMetadata {
    index_dataset: String,
    row_count: usize,
    schema: SchemaRef,
    columns: Vec<H5adObsColumn>,
}

pub struct H5adObsToDataFrameNode {
    meta: NodePorts,
    path: Option<String>,
    include_obsm: Vec<String>,
    batch_size: usize,
    validate_unique_ids: bool,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_optional_input_port_of_type(PortType::File)
        .add_output_port(None)
}

pub struct H5adObsToDataFrameNodeFactory {}

impl NodeFactory for H5adObsToDataFrameNodeFactory {
    fn kind(&self) -> &'static str {
        H5AD_OBS_TO_DATAFRAME_KIND
    }

    fn desc(&self) -> &'static str {
        "Streams AnnData H5AD /obs metadata into a DataFusion DataFrame."
    }

    fn doc(&self) -> &'static str {
        "Reads an H5AD `/obs` group and optional selected `/obsm` matrices as a \
        lazily executed DataFrame. The first column is `cell_id` derived from the \
        obs index. Expression matrices are never loaded. Supported obs encodings \
        are numeric arrays, booleans, strings, string-valued categoricals, and nullable \
        integer/boolean/string arrays. An upstream H5AD File or `path` is required; \
        `vfs://` inputs are streamed to a local temporary file for HDF5 random access."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(H5adObsToDataFrameSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: H5adObsToDataFrameSpec = serde_json::from_value(spec)?;
        if spec.batch_size == 0 {
            return Err("batch_size must be greater than zero".into());
        }
        if spec.batch_size > 1_048_576 {
            return Err("batch_size must be at most 1048576".into());
        }
        if spec.include_obsm.iter().any(|key| key.trim().is_empty()) {
            return Err("include_obsm entries cannot be empty".into());
        }
        if spec.include_obsm.len() != spec.include_obsm.iter().collect::<HashSet<_>>().len() {
            return Err("include_obsm entries must be unique".into());
        }
        Ok(Box::new(H5adObsToDataFrameNode {
            meta: port_layout(),
            path: spec.path,
            include_obsm: spec.include_obsm,
            batch_size: spec.batch_size,
            validate_unique_ids: spec.validate_unique_ids,
        }))
    }
}

#[derive(Debug)]
struct H5adObsPartition {
    path: PathBuf,
    metadata: Arc<H5adObsMetadata>,
    batch_size: usize,
    /// Keeps a staged VFS object alive for the lifetime of the lazy DataFrame.
    _temporary: Option<Arc<NamedTempFile>>,
}

impl PartitionStream for H5adObsPartition {
    fn schema(&self) -> &SchemaRef {
        &self.metadata.schema
    }

    fn execute(
        &self,
        _ctx: Arc<datafusion::execution::TaskContext>,
    ) -> datafusion::physical_plan::SendableRecordBatchStream {
        let partition = Arc::new(H5adObsPartition {
            path: self.path.clone(),
            metadata: Arc::clone(&self.metadata),
            batch_size: self.batch_size,
            _temporary: self._temporary.clone(),
        });
        let mut builder = RecordBatchReceiverStreamBuilder::new(
            Arc::clone(&self.metadata.schema),
            CHANNEL_CAPACITY,
        );
        let tx = builder.tx();
        builder.spawn_blocking(move || {
            ensure_lzf_filter().map_err(datafusion::error::DataFusionError::Execution)?;
            let file = File::open(&partition.path).map_err(|error| {
                datafusion::error::DataFusionError::Execution(format!(
                    "cannot open H5AD `{}`: {error}",
                    partition.path.display()
                ))
            })?;
            let mut offset = 0_usize;
            while offset < partition.metadata.row_count {
                let end = (offset + partition.batch_size).min(partition.metadata.row_count);
                let batch = read_batch(&file, &partition.metadata, offset, end)?;
                if tx.blocking_send(Ok(batch)).is_err() {
                    break;
                }
                offset = end;
            }
            Ok(())
        });
        builder.build()
    }
}

#[async_trait]
impl DagNode for H5adObsToDataFrameNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            meta: self.meta.clone(),
            path: self.path.clone(),
            include_obsm: self.include_obsm.clone(),
            batch_size: self.batch_size,
            validate_unique_ids: self.validate_unique_ids,
        })
    }

    fn kind(&self) -> &'static str {
        H5AD_OBS_TO_DATAFRAME_KIND
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
        let upstream = inputs.first().and_then(|input| input.file_value().ok());
        let source_path = upstream
            .map(|file| file.path.clone())
            .or_else(|| self.path.clone())
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "{H5AD_OBS_TO_DATAFRAME_KIND} requires an upstream H5AD File or path"
                ))
            })?;
        if let Some(file) = &upstream
            && let Some(format) = file.format.as_deref()
            && !format.eq_ignore_ascii_case("h5ad")
            && !format.eq_ignore_ascii_case("hdf5")
        {
            return Err(DagError::Schedule(format!(
                "{H5AD_OBS_TO_DATAFRAME_KIND} requires an H5AD File input, got `{format}`"
            )));
        }

        let mut temporary = None;
        let local_path = if source_path.starts_with("vfs://") {
            let virtual_path = match source_path.strip_prefix("vfs://") {
                Some(virtual_path) => vfs::OpendalFileStorage::normalize_path(virtual_path),
                None => source_path.clone(),
            };
            temporary = Some(Arc::new(
                stage_vfs_object(node_ctx, &virtual_path)
                    .await
                    .map_err(DagError::Schedule)?,
            ));
            temporary
                .as_ref()
                .expect("temporary H5AD was just staged")
                .as_ref()
                .path()
                .to_path_buf()
        } else {
            let local = source_path.strip_prefix("file://").unwrap_or(&source_path);
            let path = Path::new(local);
            if !path.is_file() {
                return Err(DagError::Schedule(format!(
                    "{H5AD_OBS_TO_DATAFRAME_KIND} input is not a readable file: `{local}`"
                )));
            }
            path.to_path_buf()
        };

        let include_obsm = self.include_obsm.clone();
        let validate = self.validate_unique_ids;
        let inspect =
            tokio::task::spawn_blocking(move || inspect_h5ad(local_path, &include_obsm, validate))
                .await
                .map_err(|error| {
                    DagError::Schedule(format!("H5AD inspection task failed: {error}"))
                })?
                .map_err(DagError::Schedule)?;

        let partition = H5adObsPartition {
            path: inspect.path,
            metadata: Arc::new(inspect.metadata),
            batch_size: self.batch_size,
            _temporary: temporary,
        };
        let provider = StreamingTable::try_new(
            Arc::clone(&partition.metadata.schema),
            vec![Arc::new(partition)],
        )
        .map_err(|error| DagError::Schedule(format!("cannot create H5AD table: {error}")))?;
        let dataframe = node_ctx
            .session()
            .read_table(Arc::new(provider))
            .map_err(|error| DagError::Schedule(format!("cannot read H5AD table: {error}")))?;

        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

struct InspectedPath {
    path: PathBuf,
    metadata: H5adObsMetadata,
}

async fn stage_vfs_object(node_ctx: &NodeCtx, virtual_path: &str) -> Result<NamedTempFile, String> {
    let storage = node_ctx.opendal.as_ref().ok_or_else(|| {
        format!("virtual H5AD input `{virtual_path}` requires registered object storage")
    })?;
    let length = storage
        .content_length(virtual_path)
        .await
        .map_err(|error| format!("cannot stat H5AD `{virtual_path}`: {error}"))?;
    let stream = storage
        .read_stream(virtual_path, 0..length)
        .await
        .map_err(|error| format!("cannot open H5AD `{virtual_path}`: {error}"))?;
    let mut stream = Box::pin(stream);
    let mut temporary =
        NamedTempFile::new().map_err(|error| format!("cannot create temporary H5AD: {error}"))?;
    let mut written = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("cannot read H5AD `{virtual_path}`: {error}"))?;
        temporary
            .as_file_mut()
            .write_all(&chunk)
            .map_err(|error| format!("cannot stage H5AD `{virtual_path}`: {error}"))?;
        written += chunk.len() as u64;
    }
    if written != length {
        return Err(format!(
            "H5AD `{virtual_path}` changed size while staging: expected {length} bytes, got {written}"
        ));
    }
    temporary
        .as_file_mut()
        .flush()
        .map_err(|error| format!("cannot flush staged H5AD `{virtual_path}`: {error}"))?;
    Ok(temporary)
}

fn inspect_h5ad(
    path: PathBuf,
    include_obsm: &[String],
    validate_unique_ids: bool,
) -> Result<InspectedPath, String> {
    ensure_lzf_filter()?;
    let file = File::open(&path)
        .map_err(|error| format!("cannot open H5AD `{}`: {error}", path.display()))?;
    let obs = file
        .group("obs")
        .map_err(|error| format!("H5AD has no readable `/obs` group: {error}"))?;
    let index_dataset = string_attribute(&obs, "_index")?;
    let index = dataset_1d(&obs, &index_dataset, "obs index")?;
    let row_count = index.shape().first().copied().unwrap_or(0);
    ensure_string_dataset(&index, "obs index")?;

    let mut columns = vec![H5adObsColumn {
        name: "cell_id".into(),
        source: ColumnSource::Index,
    }];
    let mut names = vec!["cell_id".to_string()];
    for column_name in string_array_attribute(&obs, "column-order")? {
        if column_name == "cell_id" {
            return Err("obs already contains a reserved `cell_id` column".into());
        }
        let (field, decoder) = inspect_obs_column(&obs, &column_name, row_count)?;
        columns.push(H5adObsColumn {
            name: column_name.clone(),
            source: ColumnSource::Obs {
                name: column_name,
                decoder,
            },
        });
        names.push(field.name().to_string());
    }

    for key in include_obsm {
        let obsm = file
            .group("obsm")
            .and_then(|group| group.dataset(key))
            .map_err(|_| format!("obsm key `{key}` is not present as a numeric matrix"))?;
        let shape = obsm.shape();
        if shape.len() != 2 || shape.first().copied().unwrap_or(0) != row_count {
            return Err(format!(
                "obsm key `{key}` is not a cell-aligned two-dimensional matrix"
            ));
        }
        let descriptor = obsm
            .dtype()
            .map_err(error)?
            .to_descriptor()
            .map_err(error)?;
        let Some(dtype) = NumericType::from_descriptor(&descriptor) else {
            return Err(format!(
                "obsm key `{key}` has an unsupported non-numeric type"
            ));
        };
        for dimension in 0..shape[1] {
            let name = safe_projection_column(key, dimension)?;
            if names.contains(&name) {
                return Err(format!(
                    "projected obsm column `{name}` collides with an existing column"
                ));
            }
            names.push(name.clone());
            columns.push(H5adObsColumn {
                name,
                source: ColumnSource::Obsm {
                    key: key.clone(),
                    dimension,
                    dtype,
                },
            });
        }
    }

    let schema = Arc::new(Schema::new(
        columns
            .iter()
            .map(|column| Field::new(&column.name, output_type(&column.source), true))
            .collect::<Vec<_>>(),
    ));
    if validate_unique_ids {
        validate_unique_obs_ids(&index, row_count, DEFAULT_BATCH_SIZE)?;
    }

    Ok(InspectedPath {
        path,
        metadata: H5adObsMetadata {
            index_dataset,
            row_count,
            schema,
            columns,
        },
    })
}

fn output_type(source: &ColumnSource) -> DataType {
    match source {
        ColumnSource::Index => DataType::Utf8,
        ColumnSource::Obs { decoder, .. } => match decoder {
            ObsDecoder::Numeric(dtype) | ObsDecoder::NullableNumeric(dtype) => dtype.arrow_type(),
            ObsDecoder::Boolean | ObsDecoder::NullableBoolean => DataType::Boolean,
            ObsDecoder::String | ObsDecoder::NullableString => DataType::Utf8,
            ObsDecoder::Categorical { .. } => {
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
            }
        },
        ColumnSource::Obsm { dtype, .. } => dtype.arrow_type(),
    }
}

fn inspect_obs_column(
    obs: &Group,
    name: &str,
    row_count: usize,
) -> Result<(Field, ObsDecoder), String> {
    let encoding = string_attribute_if_present(obs, name, "encoding-type")?;
    if let Some(encoding) = encoding.as_deref() {
        match encoding {
            "categorical" => {
                let group = obs.group(name).map_err(error)?;
                let codes = group.dataset("codes").map_err(error)?;
                let categories_dataset = group.dataset("categories").map_err(error)?;
                ensure_string_dataset(&categories_dataset, "categorical categories")?;
                let categories = read_all_strings(&categories_dataset)?;
                let codes_descriptor = codes
                    .dtype()
                    .map_err(error)?
                    .to_descriptor()
                    .map_err(error)?;
                NumericType::from_descriptor(&codes_descriptor).ok_or_else(|| {
                    format!("obs column `{name}` has non-numeric categorical codes")
                })?;
                return Ok((
                    Field::new(
                        name,
                        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                        true,
                    ),
                    ObsDecoder::Categorical { categories },
                ));
            }
            "nullable-integer" | "nullable-boolean" | "nullable-string-array" => {
                let group = obs.group(name).map_err(error)?;
                let values = group.dataset("values").map_err(error)?;
                let mask = group.dataset("mask").map_err(error)?;
                ensure_1d_length(&values, row_count, "nullable values")?;
                ensure_1d_length(&mask, row_count, "nullable mask")?;
                if !matches!(
                    mask.dtype()
                        .map_err(error)?
                        .to_descriptor()
                        .map_err(error)?,
                    TypeDescriptor::Boolean
                ) {
                    return Err(format!(
                        "obs column `{name}` has a non-boolean nullable mask"
                    ));
                }
                let value_descriptor = values
                    .dtype()
                    .map_err(error)?
                    .to_descriptor()
                    .map_err(error)?;
                let decoder = match encoding {
                    "nullable-integer" => ObsDecoder::NullableNumeric(
                        NumericType::from_descriptor(&value_descriptor).ok_or_else(|| {
                            format!("obs column `{name}` has unsupported nullable values")
                        })?,
                    ),
                    "nullable-boolean" => {
                        if !matches!(value_descriptor, TypeDescriptor::Boolean) {
                            return Err(format!(
                                "obs column `{name}` has non-boolean nullable values"
                            ));
                        }
                        ObsDecoder::NullableBoolean
                    }
                    _ => {
                        ensure_string_dataset(&values, "nullable string values")?;
                        ObsDecoder::NullableString
                    }
                };
                let data_type = output_type(&ColumnSource::Obs {
                    name: name.to_string(),
                    decoder: decoder.clone(),
                });
                return Ok((Field::new(name, data_type, true), decoder));
            }
            "array" | "string-array" => {}
            other => {
                return Err(format!(
                    "obs column `{name}` uses unsupported encoding `{other}`"
                ));
            }
        }
    }

    let dataset = dataset_1d(obs, name, "obs column")?;
    ensure_1d_length(&dataset, row_count, "obs column")?;
    let descriptor = dataset
        .dtype()
        .map_err(error)?
        .to_descriptor()
        .map_err(error)?;
    let decoder =
        match descriptor {
            TypeDescriptor::Boolean => ObsDecoder::Boolean,
            TypeDescriptor::Integer(_) | TypeDescriptor::Unsigned(_) | TypeDescriptor::Float(_) => {
                ObsDecoder::Numeric(NumericType::from_descriptor(&descriptor).ok_or_else(|| {
                    format!("obs column `{name}` has an unsupported numeric width")
                })?)
            }
            TypeDescriptor::VarLenAscii | TypeDescriptor::VarLenUnicode => ObsDecoder::String,
            other => {
                return Err(format!(
                    "obs column `{name}` has unsupported HDF5 type `{other:?}`"
                ));
            }
        };
    let data_type = output_type(&ColumnSource::Obs {
        name: name.to_string(),
        decoder: decoder.clone(),
    });
    Ok((Field::new(name, data_type, true), decoder))
}

fn read_batch(
    file: &File,
    metadata: &H5adObsMetadata,
    start: usize,
    end: usize,
) -> Result<RecordBatch, datafusion::error::DataFusionError> {
    let obs = file.group("obs").map_err(|error| {
        datafusion::error::DataFusionError::Execution(format!("cannot open `/obs`: {error}"))
    })?;
    let obsm = if metadata
        .columns
        .iter()
        .any(|column| matches!(column.source, ColumnSource::Obsm { .. }))
    {
        Some(file.group("obsm").map_err(|error| {
            datafusion::error::DataFusionError::Execution(format!("cannot open `/obsm`: {error}"))
        })?)
    } else {
        None
    };
    let mut arrays = Vec::with_capacity(metadata.columns.len());
    for column in &metadata.columns {
        let array = match &column.source {
            ColumnSource::Index => {
                let dataset = obs.dataset(&metadata.index_dataset).map_err(hdf5_error)?;
                read_string_array(&dataset, start, end, None)
            }
            ColumnSource::Obs { name, decoder } => read_obs_column(&obs, name, decoder, start, end),
            ColumnSource::Obsm {
                key,
                dimension,
                dtype,
            } => {
                let dataset = obsm
                    .as_ref()
                    .expect("obsm group is opened")
                    .dataset(key)
                    .map_err(hdf5_error)?;
                read_obsm_column(&dataset, *dimension, *dtype, start, end)
            }
        }?;
        arrays.push(array);
    }
    RecordBatch::try_new(Arc::clone(&metadata.schema), arrays).map_err(|error| {
        datafusion::error::DataFusionError::Execution(format!("invalid H5AD record batch: {error}"))
    })
}

fn hdf5_error(error: hdf5::Error) -> datafusion::error::DataFusionError {
    datafusion::error::DataFusionError::Execution(error.to_string())
}

/// Register an LZF decoder without constructing a slice from a null callback
/// pointer. This is compatible with the LZF chunks emitted by h5py/anndata.
fn ensure_lzf_filter() -> Result<(), String> {
    static REGISTRATION: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    REGISTRATION
        .get_or_init(|| unsafe {
            let info = Box::new(H5Z_class2_t {
                version: H5Z_CLASS_T_VERS as i32,
                id: LZF_FILTER_ID,
                encoder_present: 1,
                decoder_present: 1,
                name: LZF_FILTER_NAME.as_ptr().cast(),
                can_apply: None,
                set_local: Some(set_local_lzf),
                filter: Some(filter_lzf),
            });
            let ret = H5Zregister(Box::into_raw(info).cast());
            if ret < 0 {
                Err("cannot register the H5AD LZF filter".into())
            } else {
                Ok(())
            }
        })
        .clone()
}

extern "C" fn set_local_lzf(dcpl_id: hid_t, type_id: hid_t, _space_id: hid_t) -> herr_t {
    let mut chunk_dimensions = [0_u64; 32];
    let rank = unsafe { H5Pget_chunk(dcpl_id, 32, chunk_dimensions.as_mut_ptr()) };
    if rank < 0 || rank as usize > chunk_dimensions.len() {
        return -1;
    }
    let mut chunk_size = unsafe { H5Tget_size(type_id) };
    if chunk_size == 0 {
        return -1;
    }
    for dimension in &chunk_dimensions[..rank as usize] {
        chunk_size = chunk_size.saturating_mul(*dimension as usize);
    }
    if chunk_size > u32::MAX as usize {
        return -1;
    }
    let values = [LZF_FILTER_VERSION, LZF_VERSION, chunk_size as u32];
    let result = unsafe { H5Pmodify_filter(dcpl_id, LZF_FILTER_ID, 0, 3, values.as_ptr()) };
    if result < 0 { -1 } else { 1 }
}

extern "C" fn filter_lzf(
    flags: u32,
    cd_nelmts: usize,
    cd_values: *const u32,
    nbytes: usize,
    buf_size: *mut usize,
    buf: *mut *mut std::ffi::c_void,
) -> usize {
    if flags & H5Z_FLAG_REVERSE == 0 {
        unsafe { filter_lzf_compress(nbytes, buf_size, buf) }
    } else {
        unsafe { filter_lzf_decompress(cd_nelmts, cd_values, nbytes, buf_size, buf) }
    }
}

unsafe fn filter_lzf_compress(
    nbytes: usize,
    buf_size: *mut usize,
    buf: *mut *mut std::ffi::c_void,
) -> usize {
    let status = unsafe {
        let capacity = *buf_size;
        let output = libc::malloc(capacity);
        if output.is_null() {
            return 0;
        }
        let status = lzf_compress(*buf, nbytes as u32, output, capacity as u32);
        if status == 0 {
            libc::free(output);
        } else {
            libc::free(*buf);
            *buf = output;
        }
        status
    };
    status as usize
}

unsafe fn filter_lzf_decompress(
    cd_nelmts: usize,
    cd_values: *const u32,
    nbytes: usize,
    buf_size: *mut usize,
    buf: *mut *mut std::ffi::c_void,
) -> usize {
    let configured_size = unsafe {
        if cd_nelmts >= 3 && !cd_values.is_null() {
            *cd_values.add(2) as usize
        } else {
            *buf_size
        }
    };
    let mut output_size = configured_size.max(1);
    loop {
        let result = unsafe {
            let output = libc::malloc(output_size);
            if output.is_null() {
                return 0;
            }
            let status = lzf_decompress(*buf, nbytes as u32, output, output_size as u32);
            if status != 0 {
                libc::free(*buf);
                *buf = output;
                *buf_size = output_size;
                Some(status)
            } else {
                libc::free(output);
                None
            }
        };
        if let Some(status) = result {
            return status as usize;
        }
        if errno::errno().0 == 7 && output_size < usize::MAX / 2 {
            output_size += unsafe { (*buf_size).max(1) };
            continue;
        }
        return 0;
    }
}

fn read_obs_column(
    obs: &Group,
    name: &str,
    decoder: &ObsDecoder,
    start: usize,
    end: usize,
) -> Result<ArrayRef, datafusion::error::DataFusionError> {
    match decoder {
        ObsDecoder::Numeric(dtype) => {
            let dataset = obs.dataset(name).map_err(hdf5_error)?;
            read_numeric_array(&dataset, *dtype, start, end, None)
        }
        ObsDecoder::Boolean => {
            let dataset = obs.dataset(name).map_err(hdf5_error)?;
            let values = dataset
                .read_slice_1d::<bool, _>(start..end)
                .map_err(hdf5_error)?;
            Ok(Arc::new(BooleanArray::from_iter(values.iter().copied())))
        }
        ObsDecoder::String => {
            let dataset = obs.dataset(name).map_err(hdf5_error)?;
            read_string_array(&dataset, start, end, None)
        }
        ObsDecoder::NullableNumeric(dtype) => {
            let group = obs.group(name).map_err(hdf5_error)?;
            let values = group.dataset("values").map_err(hdf5_error)?;
            let mask = group.dataset("mask").map_err(hdf5_error)?;
            read_numeric_array(&values, *dtype, start, end, Some((&mask, start, end)))
        }
        ObsDecoder::NullableBoolean => {
            let group = obs.group(name).map_err(hdf5_error)?;
            let values = group.dataset("values").map_err(hdf5_error)?;
            let mask = group
                .dataset("mask")
                .map_err(hdf5_error)?
                .read_slice_1d::<bool, _>(start..end)
                .map_err(hdf5_error)?;
            let values = values
                .read_slice_1d::<bool, _>(start..end)
                .map_err(hdf5_error)?;
            Ok(Arc::new(BooleanArray::from_iter(
                values
                    .iter()
                    .zip(mask.iter())
                    .map(|(value, missing)| if *missing { None } else { Some(*value) }),
            )))
        }
        ObsDecoder::NullableString => {
            let group = obs.group(name).map_err(hdf5_error)?;
            let values = group.dataset("values").map_err(hdf5_error)?;
            let mask = group.dataset("mask").map_err(hdf5_error)?;
            read_string_array(&values, start, end, Some((&mask, start, end)))
        }
        ObsDecoder::Categorical { categories } => {
            let group = obs.group(name).map_err(hdf5_error)?;
            let codes = group.dataset("codes").map_err(hdf5_error)?;
            let descriptor = codes
                .dtype()
                .map_err(hdf5_error)?
                .to_descriptor()
                .map_err(hdf5_error)?;
            let mut builder = StringDictionaryBuilder::<Int32Type>::new();
            macro_rules! append_codes {
                ($ty:ty) => {{
                    let codes = codes
                        .read_slice_1d::<$ty, _>(start..end)
                        .map_err(hdf5_error)?;
                    for code in codes.iter() {
                        if *code < 0 {
                            builder.append_null();
                        } else {
                            let category = categories
                                .get(*code as usize)
                                .map(String::as_str)
                                .ok_or_else(|| {
                                    datafusion::error::DataFusionError::Execution(format!(
                                        "categorical code {code} is outside obs column `{name}`"
                                    ))
                                })?;
                            builder.append_value(category);
                        }
                    }
                }};
            }
            match NumericType::from_descriptor(&descriptor) {
                Some(NumericType::I8) => append_codes!(i8),
                Some(NumericType::I16) => append_codes!(i16),
                Some(NumericType::I32) => append_codes!(i32),
                Some(NumericType::I64) => append_codes!(i64),
                _ => {
                    return Err(datafusion::error::DataFusionError::Execution(format!(
                        "obs column `{name}` has unsupported categorical codes"
                    )));
                }
            }
            let values: DictionaryArray<Int32Type> = builder.finish();
            Ok(Arc::new(values))
        }
    }
}

fn read_numeric_array(
    dataset: &Dataset,
    dtype: NumericType,
    start: usize,
    end: usize,
    mask: Option<(&Dataset, usize, usize)>,
) -> Result<ArrayRef, datafusion::error::DataFusionError> {
    macro_rules! read_values {
        ($ty:ty, $marker:ty) => {{
            let values = dataset
                .read_slice_1d::<$ty, _>(start..end)
                .map_err(hdf5_error)?;
            if let Some((mask_dataset, mask_start, mask_end)) = mask {
                let mask = mask_dataset
                    .read_slice_1d::<bool, _>(mask_start..mask_end)
                    .map_err(hdf5_error)?;
                let mut builder = PrimitiveBuilder::<$marker>::with_capacity(end - start);
                for (value, missing) in values.iter().zip(mask.iter()) {
                    builder.append_option(if *missing { None } else { Some(*value) });
                }
                Arc::new(builder.finish())
            } else {
                Arc::new(PrimitiveArray::<$marker>::from_iter(values.iter().copied()))
            }
        }};
    }
    Ok(match dtype {
        NumericType::I8 => read_values!(i8, Int8Type),
        NumericType::I16 => read_values!(i16, Int16Type),
        NumericType::I32 => read_values!(i32, Int32Type),
        NumericType::I64 => read_values!(i64, Int64Type),
        NumericType::U8 => read_values!(u8, UInt8Type),
        NumericType::U16 => read_values!(u16, UInt16Type),
        NumericType::U32 => read_values!(u32, UInt32Type),
        NumericType::U64 => read_values!(u64, UInt64Type),
        NumericType::F32 => read_values!(f32, Float32Type),
        NumericType::F64 => read_values!(f64, Float64Type),
    })
}

fn read_string_array(
    dataset: &Dataset,
    start: usize,
    end: usize,
    mask: Option<(&Dataset, usize, usize)>,
) -> Result<ArrayRef, datafusion::error::DataFusionError> {
    let descriptor = dataset
        .dtype()
        .map_err(hdf5_error)?
        .to_descriptor()
        .map_err(hdf5_error)?;
    let mut values = match descriptor {
        TypeDescriptor::VarLenAscii => dataset
            .read_slice_1d::<VarLenAscii, _>(start..end)
            .map_err(hdf5_error)?
            .iter()
            .map(|value| value.as_str().to_string())
            .collect::<Vec<_>>(),
        TypeDescriptor::VarLenUnicode => dataset
            .read_slice_1d::<VarLenUnicode, _>(start..end)
            .map_err(hdf5_error)?
            .iter()
            .map(|value| value.as_str().to_string())
            .collect::<Vec<_>>(),
        other => {
            return Err(datafusion::error::DataFusionError::Execution(format!(
                "unsupported H5AD string type `{other:?}`"
            )));
        }
    };
    let mask = match mask {
        Some((mask_dataset, mask_start, mask_end)) => Some(
            mask_dataset
                .read_slice_1d::<bool, _>(mask_start..mask_end)
                .map_err(hdf5_error)?,
        ),
        None => None,
    };
    let mut builder = arrow_array::builder::StringBuilder::new();
    for (index, value) in values.drain(..).enumerate() {
        let missing = mask
            .as_ref()
            .and_then(|mask| mask.get(index).copied())
            .unwrap_or(false);
        if missing {
            builder.append_null();
        } else {
            builder.append_value(value);
        }
    }
    Ok(Arc::new(StringArray::from(builder.finish())))
}

fn read_obsm_column(
    dataset: &Dataset,
    dimension: usize,
    dtype: NumericType,
    start: usize,
    end: usize,
) -> Result<ArrayRef, datafusion::error::DataFusionError> {
    macro_rules! read_embedding {
        ($ty:ty, $marker:ty) => {{
            let values = dataset
                .read_slice_2d::<$ty, _>(s![start..end, dimension..dimension + 1])
                .map_err(hdf5_error)?;
            Arc::new(PrimitiveArray::<$marker>::from_iter(
                values.column(0).iter().copied(),
            ))
        }};
    }
    Ok(match dtype {
        NumericType::I8 => read_embedding!(i8, Int8Type),
        NumericType::I16 => read_embedding!(i16, Int16Type),
        NumericType::I32 => read_embedding!(i32, Int32Type),
        NumericType::I64 => read_embedding!(i64, Int64Type),
        NumericType::U8 => read_embedding!(u8, UInt8Type),
        NumericType::U16 => read_embedding!(u16, UInt16Type),
        NumericType::U32 => read_embedding!(u32, UInt32Type),
        NumericType::U64 => read_embedding!(u64, UInt64Type),
        NumericType::F32 => read_embedding!(f32, Float32Type),
        NumericType::F64 => read_embedding!(f64, Float64Type),
    })
}

fn string_attribute(group: &Group, name: &str) -> Result<String, String> {
    let attribute = group
        .attr(name)
        .map_err(|_| format!("H5AD group is missing `{name}` attribute"))?;
    let values = attribute
        .as_reader()
        .read_raw::<VarLenUnicode>()
        .map_err(|error| format!("cannot read `{name}` attribute: {error}"))?;
    values
        .first()
        .map(|value| value.as_str().to_string())
        .ok_or_else(|| format!("`{name}` attribute is empty"))
}

fn string_attribute_if_present(
    group: &Group,
    object: &str,
    attribute: &str,
) -> Result<Option<String>, String> {
    let candidate = if let Ok(member) = group.group(object) {
        member.attr(attribute).ok()
    } else if let Ok(member) = group.dataset(object) {
        member.attr(attribute).ok()
    } else {
        return Err(format!("obs member `{object}` cannot be opened"));
    };
    let Some(attr) = candidate else {
        return Ok(None);
    };
    let values = attr
        .as_reader()
        .read_raw::<VarLenUnicode>()
        .map_err(|err| format!("cannot read `{attribute}` attribute: {err}"))?;
    values
        .first()
        .map(|value| value.as_str().to_string())
        .map(Some)
        .ok_or_else(|| format!("`{attribute}` attribute is empty"))
}

fn string_array_attribute(group: &Group, name: &str) -> Result<Vec<String>, String> {
    let attribute = group
        .attr(name)
        .map_err(|err| format!("H5AD group is missing `{name}` attribute: {err}"))?;
    let values = attribute
        .as_reader()
        .read_raw::<VarLenUnicode>()
        .map_err(|err| format!("cannot read `{name}` attribute: {err}"))?;
    Ok(values
        .into_iter()
        .map(|value| value.as_str().to_string())
        .collect())
}

fn dataset_1d(group: &Group, name: &str, label: &str) -> Result<Dataset, String> {
    let dataset = group
        .dataset(name)
        .map_err(|error| format!("`{label}` `{name}` is not a dataset: {error}"))?;
    let shape = dataset.shape();
    if shape.len() != 1 {
        return Err(format!("`{label}` `{name}` must be one-dimensional"));
    }
    Ok(dataset)
}

fn ensure_1d_length(dataset: &Dataset, row_count: usize, label: &str) -> Result<(), String> {
    let shape = dataset.shape();
    if shape.as_slice() != [row_count] {
        return Err(format!(
            "`{label}` must contain exactly {row_count} values, found shape {shape:?}"
        ));
    }
    Ok(())
}

fn ensure_string_dataset(dataset: &Dataset, label: &str) -> Result<(), String> {
    match dataset
        .dtype()
        .map_err(error)?
        .to_descriptor()
        .map_err(error)?
    {
        TypeDescriptor::VarLenAscii | TypeDescriptor::VarLenUnicode => Ok(()),
        other => Err(format!(
            "`{label}` must use a variable-length UTF-8 string type, found {other:?}"
        )),
    }
}

fn read_all_strings(dataset: &Dataset) -> Result<Vec<String>, String> {
    match dataset
        .dtype()
        .map_err(error)?
        .to_descriptor()
        .map_err(error)?
    {
        TypeDescriptor::VarLenAscii => {
            dataset
                .read_1d::<VarLenAscii>()
                .map_err(error)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().to_string())
                        .collect()
                })
        }
        TypeDescriptor::VarLenUnicode => {
            dataset
                .read_1d::<VarLenUnicode>()
                .map_err(error)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().to_string())
                        .collect()
                })
        }
        other => Err(format!("unsupported string dataset type `{other:?}`")),
    }
}

fn validate_unique_obs_ids(
    dataset: &Dataset,
    row_count: usize,
    batch_size: usize,
) -> Result<(), String> {
    let mut unique = HashSet::new();
    let mut offset = 0_usize;
    while offset < row_count {
        let end = (offset + batch_size).min(row_count);
        for value in read_all_string_slice(dataset, offset, end)? {
            if !unique.insert(value) {
                return Err("H5AD contract violation: obs_names are not unique".into());
            }
        }
        offset = end;
    }
    Ok(())
}

fn read_all_string_slice(
    dataset: &Dataset,
    start: usize,
    end: usize,
) -> Result<Vec<String>, String> {
    match dataset
        .dtype()
        .map_err(error)?
        .to_descriptor()
        .map_err(error)?
    {
        TypeDescriptor::VarLenAscii => dataset
            .read_slice_1d::<VarLenAscii, _>(start..end)
            .map_err(error)
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().to_string())
                    .collect()
            }),
        TypeDescriptor::VarLenUnicode => dataset
            .read_slice_1d::<VarLenUnicode, _>(start..end)
            .map_err(error)
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().to_string())
                    .collect()
            }),
        other => Err(format!("unsupported obs index string type `{other:?}`")),
    }
}

fn safe_projection_column(key: &str, dimension: usize) -> Result<String, String> {
    let safe = key
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if safe.is_empty() {
        return Err(format!(
            "obsm key `{key}` cannot be projected to a column name"
        ));
    }
    Ok(format!("obsm_{safe}_{dimension}"))
}

fn error(error: hdf5::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Array;
    use hdf5::types::VarLenUnicode;
    use std::str::FromStr;
    use vfs::OpendalFileStorage;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    fn vfs_node_ctx(storage: &Arc<OpendalFileStorage>) -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(Arc::clone(storage)),
        )
    }

    fn unicode(value: &str) -> VarLenUnicode {
        VarLenUnicode::from_str(value).unwrap()
    }

    fn write_string_dataset(group: &Group, name: &str, values: &[&str]) -> hdf5::Result<()> {
        let values = values
            .iter()
            .map(|value| unicode(value))
            .collect::<Vec<_>>();
        let dataset = group
            .new_dataset::<VarLenUnicode>()
            .shape(values.len())
            .create(name)?;
        dataset.as_writer().write_raw(&values)?;
        Ok(())
    }

    fn write_string_attribute(object: &Group, name: &str, values: &[&str]) -> hdf5::Result<()> {
        let values = values
            .iter()
            .map(|value| unicode(value))
            .collect::<Vec<_>>();
        let attribute = object
            .new_attr::<VarLenUnicode>()
            .shape(values.len())
            .create(name)?;
        attribute.as_writer().write_raw(&values)?;
        Ok(())
    }

    fn fixture(path: &Path) {
        let file = File::create(path).unwrap();
        let obs = file.create_group("obs").unwrap();
        write_string_attribute(&obs, "encoding-type", &["dataframe"]).unwrap();
        write_string_attribute(&obs, "encoding-version", &["0.2.0"]).unwrap();
        write_string_attribute(&obs, "_index", &["_index"]).unwrap();
        write_string_dataset(&obs, "_index", &["c0", "c1", "c2", "c3", "c4"]).unwrap();
        obs.new_dataset::<i32>()
            .shape(5)
            .create("count")
            .unwrap()
            .as_writer()
            .write_raw(&[1_i32, 2, 3, 4, 5])
            .unwrap();
        write_string_dataset(&obs, "label", &["a", "b", "a", "b", "a"]).unwrap();

        let categorical = obs.create_group("category").unwrap();
        write_string_attribute(&categorical, "encoding-type", &["categorical"]).unwrap();
        write_string_attribute(&categorical, "encoding-version", &["0.2.0"]).unwrap();
        categorical
            .new_attr::<bool>()
            .shape(())
            .create("ordered")
            .unwrap()
            .as_writer()
            .write_scalar(&false)
            .unwrap();
        categorical
            .new_dataset::<i8>()
            .shape(5)
            .create("codes")
            .unwrap()
            .as_writer()
            .write_raw(&[0_i8, 1, -1, 0, 1])
            .unwrap();
        write_string_dataset(&categorical, "categories", &["x", "y"]).unwrap();

        let nullable = obs.create_group("score").unwrap();
        write_string_attribute(&nullable, "encoding-type", &["nullable-integer"]).unwrap();
        write_string_attribute(&nullable, "encoding-version", &["0.1.0"]).unwrap();
        nullable
            .new_dataset::<i64>()
            .shape(5)
            .create("values")
            .unwrap()
            .as_writer()
            .write_raw(&[10_i64, 20, 30, 40, 50])
            .unwrap();
        nullable
            .new_dataset::<bool>()
            .shape(5)
            .create("mask")
            .unwrap()
            .as_writer()
            .write_raw(&[false, false, true, false, false])
            .unwrap();

        write_string_attribute(
            &obs,
            "column-order",
            &["count", "label", "category", "score"],
        )
        .unwrap();

        let obsm = file.create_group("obsm").unwrap();
        obsm.new_dataset::<f32>()
            .shape((5, 2))
            .create("X_umap")
            .unwrap()
            .as_writer()
            .write_slice(
                ndarray::Array2::from_shape_vec(
                    (5, 2),
                    vec![0.0_f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0],
                )
                .unwrap()
                .view(),
                s![.., ..],
            )
            .unwrap();
    }

    #[tokio::test]
    async fn streams_obs_and_selected_obsm_to_dataframe() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.h5ad");
        fixture(&path);
        let mut node = H5adObsToDataFrameNodeFactory {}
            .build(
                serde_json::json!({
                    "path": path.to_string_lossy(),
                    "include_obsm": ["X_umap"],
                    "batch_size": 2
                }),
                node_ctx(),
            )
            .unwrap();
        let output = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let dataframe = output.get(&0).unwrap().as_dataframe().unwrap().clone();
        let schema = dataframe.schema();
        let names = schema
            .fields()
            .iter()
            .map(|field| field.name().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "cell_id",
                "count",
                "label",
                "category",
                "score",
                "obsm_X_umap_0",
                "obsm_X_umap_1"
            ]
        );
        let batches = dataframe.collect().await.unwrap();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 5);

        let cell_ids = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(cell_ids.value(0), "c0");
        assert_eq!(cell_ids.value(1), "c1");

        let counts = batches[0]
            .column(1)
            .as_any()
            .downcast_ref::<PrimitiveArray<Int32Type>>()
            .unwrap();
        assert_eq!(counts.value(1), 2);

        let categories = batches[0]
            .column(3)
            .as_any()
            .downcast_ref::<DictionaryArray<Int32Type>>()
            .unwrap();
        assert_eq!(categories.key(0), Some(0));
        assert_eq!(categories.key(1), Some(1));
        let categories = batches[1]
            .column(3)
            .as_any()
            .downcast_ref::<DictionaryArray<Int32Type>>()
            .unwrap();
        assert!(categories.is_null(0));
        assert_eq!(categories.key(1), Some(0));

        let scores = batches[0]
            .column(4)
            .as_any()
            .downcast_ref::<PrimitiveArray<Int64Type>>()
            .unwrap();
        assert_eq!(scores.value(1), 20);
        let scores = batches[1]
            .column(4)
            .as_any()
            .downcast_ref::<PrimitiveArray<Int64Type>>()
            .unwrap();
        assert!(scores.is_null(0));
        assert_eq!(scores.value(1), 40);

        let umap = batches[0]
            .column(5)
            .as_any()
            .downcast_ref::<PrimitiveArray<Float32Type>>()
            .unwrap();
        assert_eq!(umap.value(1), 2.0);
    }

    #[tokio::test]
    async fn reads_real_anndata_fixture_when_provided() {
        let Ok(path) = std::env::var("AUTONOMICS_H5AD_OBS_TEST_INPUT") else {
            return;
        };
        let mut node = H5adObsToDataFrameNodeFactory {}
            .build(
                serde_json::json!({
                    "path": path,
                    "include_obsm": ["X_emb"],
                    "batch_size": 2
                }),
                node_ctx(),
            )
            .unwrap();
        let output = node
            .execute(
                &node_ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let dataframe = output.get(&0).unwrap().as_dataframe().unwrap().clone();
        assert!(dataframe.schema().fields().len() >= 4);
        assert_eq!(dataframe.schema().field(0).name(), "cell_id");
        if dataframe.schema().fields().len() >= 8 {
            assert_eq!(dataframe.schema().field(6).name(), "obsm_X_emb_0");
        }
        let batches = dataframe.collect().await.unwrap();
        assert!(batches.iter().map(RecordBatch::num_rows).sum::<usize>() > 0);
    }

    #[tokio::test]
    async fn stages_vfs_h5ad_before_lazy_collection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.h5ad");
        fixture(&path);
        let storage = Arc::new(OpendalFileStorage::new_temp());
        storage
            .write_bytes(
                "/input.h5ad",
                std::fs::read(&path).expect("fixture is readable"),
            )
            .await
            .unwrap();
        let mut node = H5adObsToDataFrameNodeFactory {}
            .build(
                serde_json::json!({ "path": "vfs:///input.h5ad" }),
                vfs_node_ctx(&storage),
            )
            .unwrap();
        let output = node
            .execute(
                &vfs_node_ctx(&storage),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let dataframe = output.get(&0).unwrap().as_dataframe().unwrap().clone();
        let batches = dataframe.collect().await.unwrap();
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 5);
    }
}
