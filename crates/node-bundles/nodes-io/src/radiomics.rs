//! Native DataFrame nodes for the radiomics Stage-A contract.
//!
//! These nodes deliberately do not decode pixels. They normalize cohort
//! metadata, resolve concrete image/mask files, and perform deterministic
//! feature-table quality control. Pixel geometry and feature computation are
//! delegated to the containerized radiomics nodes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path as StdPath;
use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::object_store::{ObjectStore, path::Path as ObjectStorePath};
use futures::StreamExt;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileFingerprint, FileRef, PortType};
use datafusion::prelude::DataFrame;

pub const RADIOMICS_MANIFEST_KIND: &str = "radiomics_manifest_build";
pub const RADIOMICS_STAGE_FILE_SET_KIND: &str = "radiomics_stage_file_set";
pub const RADIOMICS_DCM_GLOB_KIND: &str = "radiomics_dcm_glob";
pub const RADIOMICS_QC_KIND: &str = "radiomics_qc";
pub const RADIOMICS_FEATURE_SET_ASSEMBLE_KIND: &str = "radiomics_feature_set_assemble";

const RESERVED_FEATURE_SET_COLUMNS: &[&str] = &[
    "extraction_id",
    "patient_id",
    "image_id",
    "roi_id",
    "roi_name",
    "modality",
    "preset_id",
    "timepoint",
    "feature_set_id",
    "feature_set_version",
];

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsManifestSpec {
    /// Column containing the image path or `vfs://` URI.
    #[serde(default = "default_image_column")]
    pub image_column: String,
    /// Column containing the paired mask path or `vfs://` URI.
    #[serde(default = "default_mask_column")]
    pub mask_column: String,
    /// Column containing the stable patient identifier.
    #[serde(default = "default_patient_column")]
    pub patient_id_column: String,
    /// Column containing one examination or image-series identifier.
    #[serde(default = "default_image_id_column")]
    pub image_id_column: String,
    /// Optional column containing a globally unique extraction identifier.
    #[serde(default)]
    pub extraction_id_column: Option<String>,
    /// Column containing CT, MR, PET, or OTHER.
    #[serde(default = "default_modality_column")]
    pub modality_column: String,
    /// Column containing the ROI identifier.
    #[serde(default = "default_roi_column")]
    pub roi_id_column: String,
    /// Optional human-readable ROI name column.
    #[serde(default)]
    pub roi_name_column: Option<String>,
    /// Column containing the positive integer mask label.
    #[serde(default = "default_mask_label_column")]
    pub mask_label_column: String,
    /// Optional acquisition timepoint column.
    #[serde(default)]
    pub timepoint_column: Option<String>,
    /// Column containing the extraction parameter preset identifier.
    #[serde(default = "default_preset_column")]
    pub preset_id_column: String,
    /// Verify that every image and mask resolves through local or VFS storage.
    #[serde(default = "default_true")]
    pub validate_paths: bool,
}

fn default_image_column() -> String {
    "image_uri".into()
}
fn default_mask_column() -> String {
    "mask_uri".into()
}
fn default_patient_column() -> String {
    "patient_id".into()
}
fn default_image_id_column() -> String {
    "image_id".into()
}
fn default_modality_column() -> String {
    "modality".into()
}
fn default_roi_column() -> String {
    "roi_id".into()
}
fn default_mask_label_column() -> String {
    "mask_label".into()
}
fn default_preset_column() -> String {
    "preset_id".into()
}
fn default_true() -> bool {
    true
}

pub struct RadiomicsManifestNode {
    ports: NodePorts,
    spec: RadiomicsManifestSpec,
}

fn manifest_port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(None)
        .add_output_port(None)
}

pub fn validate_manifest_spec(spec: &RadiomicsManifestSpec) -> Result<(), String> {
    let columns = [
        &spec.image_column,
        &spec.mask_column,
        &spec.patient_id_column,
        &spec.image_id_column,
        &spec.modality_column,
        &spec.roi_id_column,
        &spec.mask_label_column,
        &spec.preset_id_column,
    ];
    if columns.iter().any(|column| column.trim().is_empty()) {
        return Err("manifest column names cannot be empty".into());
    }
    let mut unique = columns
        .iter()
        .map(|column| column.as_str())
        .collect::<Vec<_>>();
    if let Some(column) = &spec.extraction_id_column {
        unique.push(column);
    }
    if let Some(column) = &spec.roi_name_column {
        unique.push(column);
    }
    if let Some(column) = &spec.timepoint_column {
        unique.push(column);
    }
    let distinct = unique.iter().collect::<BTreeSet<_>>();
    if distinct.len() != unique.len() {
        return Err("manifest column mappings must be distinct".into());
    }
    Ok(())
}

#[async_trait]
impl DagNode for RadiomicsManifestNode {
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
        RADIOMICS_MANIFEST_KIND
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
        let batches = collect_dataframe(inputs).await?;
        let source_columns = SourceColumns::from_manifest_spec(&self.spec);
        let rows = extract_manifest_rows(&batches, &source_columns)?;
        let mut seen_ids = BTreeSet::new();
        let mut outputs = ManifestRows::default();
        let mut diagnostics = Vec::new();

        for (index, row) in rows.iter().enumerate() {
            let mut errors = Vec::new();
            let image_uri = required_or_invalid(&row.image, "image URI", &mut errors);
            let mask_uri = required_or_invalid(&row.mask, "mask URI", &mut errors);
            let source_format = infer_format(&image_uri);
            let patient_id = required_or_invalid(&row.patient_id, "patient ID", &mut errors);
            let image_id = required_or_invalid(&row.image_id, "image ID", &mut errors);
            let roi_id = required_or_invalid(&row.roi_id, "ROI ID", &mut errors);
            let modality = match row.modality.as_deref() {
                Some("CT" | "MR" | "PET" | "OTHER") => {
                    row.modality.clone().expect("checked").to_uppercase()
                }
                Some(other) => {
                    errors.push(format!("unsupported modality `{other}`"));
                    "OTHER".to_string()
                }
                None => {
                    errors.push("modality is missing".into());
                    "OTHER".to_string()
                }
            };
            let mask_label = row.mask_label.unwrap_or(1.0);
            if !(mask_label.is_finite() && mask_label > 0.0 && mask_label.fract() == 0.0) {
                errors.push("mask_label must be a positive integer".into());
            }
            if image_uri == mask_uri {
                errors.push("image and mask resolve to the same URI".into());
            }

            let extraction_id = row
                .extraction_id
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| derive_extraction_id(&patient_id, &image_id, &roi_id));
            if !seen_ids.insert(extraction_id.clone()) {
                errors.push("duplicate extraction_id".into());
            }

            if self.spec.validate_paths {
                if let Err(error) = resolve_file_ref(ctx, &image_uri).await {
                    errors.push(format!("image URI is not readable: {error}"));
                }
                if let Err(error) = resolve_file_ref(ctx, &mask_uri).await {
                    errors.push(format!("mask URI is not readable: {error}"));
                }
            }

            let status = if errors.is_empty() {
                "valid"
            } else {
                "invalid"
            };
            let error_code = if errors.is_empty() {
                String::new()
            } else {
                errors.join("; ")
            };
            outputs.push(ManifestRow {
                extraction_id,
                patient_id,
                image_id,
                image_uri,
                mask_uri,
                modality,
                roi_id,
                roi_name: row.roi_name.clone().unwrap_or_default(),
                mask_label: mask_label as i32,
                timepoint: row.timepoint.clone().unwrap_or_default(),
                preset_id: row
                    .preset_id
                    .clone()
                    .unwrap_or_else(|| "pyradiomics_original_v1".into()),
                format: source_format,
                status: status.into(),
                error_code,
            });
            for error in errors {
                diagnostics.push(DiagnosticRow {
                    extraction_id: outputs
                        .rows
                        .last()
                        .expect("row was inserted")
                        .extraction_id
                        .clone(),
                    row: index as i32,
                    severity: "error".into(),
                    error_code: error.clone(),
                    message: error,
                });
            }
        }

        let session = ctx.session();
        let manifest_df = session
            .read_batch(manifest_record_batch(&outputs).map_err(arrow_error)?)
            .map_err(|error| {
                DagError::Schedule(format!("cannot create manifest DataFrame: {error}"))
            })?;
        let diagnostics_df = session
            .read_batch(diagnostics_record_batch(&diagnostics).map_err(arrow_error)?)
            .map_err(|error| {
                DagError::Schedule(format!("cannot create diagnostics DataFrame: {error}"))
            })?;
        let mut result = PortOutputs::new();
        result.insert(0, manifest_df);
        result.insert(1, diagnostics_df);
        Ok(result)
    }
}

pub struct RadiomicsManifestNodeFactory;

impl NodeFactory for RadiomicsManifestNodeFactory {
    fn kind(&self) -> &'static str {
        RADIOMICS_MANIFEST_KIND
    }

    fn desc(&self) -> &'static str {
        "Builds and validates a radiomics extraction manifest."
    }

    fn doc(&self) -> &'static str {
        "Normalizes a cohort table into the radiomics Stage-A extraction-unit \
        contract. One row represents one patient/image/ROI/label extraction, \
        not one patient. The node validates identifiers, modality, mask label, \
        image/mask uniqueness, and optionally resolves both paths through the \
        local or virtual filesystem. Port 0 emits the normalized manifest; \
        port 1 emits row-level diagnostics. Invalid rows are retained with \
        status=`invalid` so downstream agents can inspect failures."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RadiomicsManifestSpec)
    }

    fn ports(&self) -> NodePorts {
        manifest_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: RadiomicsManifestSpec = serde_json::from_value(spec)?;
        validate_manifest_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(Box::new(RadiomicsManifestNode {
            ports: manifest_port_layout(),
            spec,
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: RadiomicsManifestSpec = serde_json::from_value(spec)?;
        validate_manifest_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(manifest_port_layout())
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsStageFileSetSpec {
    /// Manifest column containing concrete image or mask paths.
    #[serde(default = "default_image_column")]
    pub column: String,
    /// Emit only rows whose manifest status is `valid`.
    #[serde(default = "default_true")]
    pub valid_only: bool,
}

pub struct RadiomicsStageFileSetNode {
    ports: NodePorts,
    column: String,
    valid_only: bool,
}

fn stage_file_set_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port_of_type(None, PortType::FileSet)
}

#[async_trait]
impl DagNode for RadiomicsStageFileSetNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            column: self.column.clone(),
            valid_only: self.valid_only,
        })
    }

    fn kind(&self) -> &'static str {
        RADIOMICS_STAGE_FILE_SET_KIND
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
        let batches = collect_dataframe(inputs).await?;
        let mut paths = Vec::new();
        for (batch_index, batch) in batches.iter().enumerate() {
            let Some(column) = batch.column_by_name(&self.column) else {
                return Err(DagError::Schedule(format!(
                    "radiomics_stage_file_set cannot find column `{}`",
                    self.column
                )));
            };
            let values = string_values(column.as_ref()).ok_or_else(|| {
                DagError::Schedule(format!(
                    "radiomics_stage_file_set column `{}` must be a string",
                    self.column
                ))
            })?;
            let status_values = batch
                .column_by_name("status")
                .and_then(|column| string_values(column.as_ref()));
            for (row_index, value) in values.iter().enumerate() {
                if self.valid_only
                    && status_values
                        .as_ref()
                        .and_then(|values| values.get(row_index))
                        .and_then(Option::as_deref)
                        .unwrap_or("valid")
                        != "valid"
                {
                    continue;
                }
                let path = value.clone().filter(|value| !value.trim().is_empty()).ok_or_else(|| {
                    DagError::Schedule(format!(
                        "radiomics_stage_file_set found an empty path at row {row_index} in batch {batch_index}"
                    ))
                })?;
                paths.push(
                    resolve_file_ref(ctx, &path)
                        .await
                        .map_err(DagError::Schedule)?,
                );
            }
        }
        if paths.is_empty() {
            return Err(DagError::Schedule(
                "radiomics_stage_file_set emitted no files".into(),
            ));
        }
        let mut outputs = PortOutputs::new();
        outputs.insert(0, paths);
        Ok(outputs)
    }
}

pub struct RadiomicsStageFileSetNodeFactory;

impl NodeFactory for RadiomicsStageFileSetNodeFactory {
    fn kind(&self) -> &'static str {
        RADIOMICS_STAGE_FILE_SET_KIND
    }

    fn desc(&self) -> &'static str {
        "Resolves manifest paths into an ordered FileSet."
    }

    fn doc(&self) -> &'static str {
        "Reads one path column from a radiomics manifest and resolves every \
        entry through local or VFS storage. The output preserves manifest row \
        order and is intended for batch container nodes that consume an image \
        FileSet and a mask FileSet. With valid_only=true, rows marked \
        status=`invalid` are skipped consistently in both FileSets."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RadiomicsStageFileSetSpec)
    }

    fn ports(&self) -> NodePorts {
        stage_file_set_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: RadiomicsStageFileSetSpec = serde_json::from_value(spec)?;
        if spec.column.trim().is_empty() {
            return Err(dag_core::registry::error::Error::Unknown(
                "column cannot be empty".into(),
            ));
        }
        Ok(Box::new(RadiomicsStageFileSetNode {
            ports: stage_file_set_ports(),
            column: spec.column,
            valid_only: spec.valid_only,
        }))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsDcmGlobSpec {
    /// Absolute local glob or `vfs://` glob, for example
    /// `/data/subject/CT/*.dcm` or `vfs:///subject/CT/*.dcm`.
    pub pattern: String,
}

pub struct RadiomicsDcmGlobNode {
    ports: NodePorts,
    pattern: String,
}

fn dcm_glob_ports() -> NodePorts {
    NodePorts::new().add_output_port_of_type(None, PortType::FileSet)
}

#[async_trait]
impl DagNode for RadiomicsDcmGlobNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            pattern: self.pattern.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        RADIOMICS_DCM_GLOB_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let files = glob_dicom_files(ctx, &self.pattern).await?;
        if files.is_empty() {
            return Err(DagError::Schedule(format!(
                "radiomics_dcm_glob pattern `{}` matched no files",
                self.pattern
            )));
        }
        let mut outputs = PortOutputs::new();
        outputs.insert(0, files);
        Ok(outputs)
    }
}

async fn glob_dicom_files(ctx: &NodeCtx, pattern: &str) -> Result<Vec<FileRef>, DagError> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(DagError::Schedule("glob pattern cannot be empty".into()));
    }

    if let Some(virtual_pattern) = trimmed.strip_prefix("vfs://") {
        let Some(storage) = ctx.opendal.as_ref() else {
            return Err(DagError::Schedule(
                "vfs:// DICOM globs require registered object storage".into(),
            ));
        };
        let virtual_pattern = vfs::OpendalFileStorage::normalize_path(virtual_pattern);
        let wildcard = virtual_pattern
            .find(['*', '?', '['])
            .unwrap_or(virtual_pattern.len());
        let base = &virtual_pattern[..wildcard];
        let prefix = base
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("/");
        let prefix = if prefix.is_empty() {
            "/".to_string()
        } else {
            prefix.to_string()
        };
        if !storage.is_mounted(&prefix) {
            return Err(DagError::Schedule(format!(
                "vfs:// glob prefix `{prefix}` is not mounted"
            )));
        }
        let glob = glob::Pattern::new(&virtual_pattern)
            .map_err(|error| DagError::Schedule(format!("invalid DICOM glob: {error}")))?;
        let object_prefix = ObjectStorePath::parse(&prefix).map_err(|error| {
            DagError::Schedule(format!("invalid VFS prefix `{prefix}`: {error}"))
        })?;
        let metas = storage.list(Some(&object_prefix)).collect::<Vec<_>>().await;
        let mut files = Vec::new();
        for meta in metas {
            let meta = meta.map_err(|error| {
                DagError::Schedule(format!("cannot list VFS prefix `{prefix}`: {error}"))
            })?;
            let path = format!("/{}", meta.location.as_ref().trim_start_matches('/'));
            if !glob.matches_path(StdPath::new(&path)) {
                continue;
            }
            let mtime_ns = meta.last_modified.timestamp_nanos_opt().unwrap_or_default();
            files.push(FileRef {
                path: format!("vfs://{path}"),
                format: Some("dicom".into()),
                fingerprint: Some(FileFingerprint {
                    size: meta.size,
                    mtime_ns: mtime_ns as i128,
                    content_hash: meta.e_tag,
                    immutable_remote: true,
                }),
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        return Ok(files);
    }

    let local_pattern = trimmed.strip_prefix("file://").unwrap_or(trimmed);
    if !StdPath::new(local_pattern).is_absolute() {
        return Err(DagError::Schedule(
            "DICOM glob must be an absolute path or vfs:// URI".into(),
        ));
    }
    let glob = glob::Pattern::new(local_pattern)
        .map_err(|error| DagError::Schedule(format!("invalid DICOM glob: {error}")))?;
    let matches = glob::glob(local_pattern)
        .map_err(|error| DagError::Schedule(format!("invalid DICOM glob: {error}")))?;
    let mut files = Vec::new();
    for path in matches {
        let path =
            path.map_err(|error| DagError::Schedule(format!("invalid DICOM glob match: {error}")))?;
        if !glob.matches_path(&path) || !path.is_file() {
            continue;
        }
        files.push(
            FileRef::local(path, Some("dicom".into()))
                .map_err(|error| DagError::Schedule(error.to_string()))?,
        );
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

pub struct RadiomicsDcmGlobNodeFactory;

impl NodeFactory for RadiomicsDcmGlobNodeFactory {
    fn kind(&self) -> &'static str {
        RADIOMICS_DCM_GLOB_KIND
    }

    fn desc(&self) -> &'static str {
        "Expands and sorts a DICOM directory or glob into a FileSet."
    }

    fn doc(&self) -> &'static str {
        "Accepts one absolute local glob or one `vfs://` glob, filters regular \
        files, sorts matches lexically, and emits a DICOM FileSet. Image \
        ingestion independently orders multi-file DICOM geometry before \
        constructing a volume."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RadiomicsDcmGlobSpec)
    }

    fn ports(&self) -> NodePorts {
        dcm_glob_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: RadiomicsDcmGlobSpec = serde_json::from_value(spec)?;
        if spec.pattern.trim().is_empty() {
            return Err(dag_core::registry::error::Error::Unknown(
                "pattern cannot be empty".into(),
            ));
        }
        Ok(Box::new(RadiomicsDcmGlobNode {
            ports: dcm_glob_ports(),
            pattern: spec.pattern,
        }))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsQcSpec {
    /// Unique extraction-unit identifier column.
    #[serde(default = "default_extraction_column")]
    pub id_column: String,
    /// Maximum fraction of missing values retained in a feature column.
    #[serde(default = "default_max_missing")]
    pub max_feature_missing_rate: f64,
    /// Remove constant feature columns.
    #[serde(default = "default_true")]
    pub drop_constant_features: bool,
}

fn default_extraction_column() -> String {
    "extraction_id".into()
}
fn default_max_missing() -> f64 {
    0.2
}

pub struct RadiomicsQcNode {
    ports: NodePorts,
    id_column: String,
    max_missing: f64,
    drop_constant: bool,
}

fn qc_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(None)
        .add_output_port(None)
        .add_output_port(None)
}

#[async_trait]
impl DagNode for RadiomicsQcNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            id_column: self.id_column.clone(),
            max_missing: self.max_missing,
            drop_constant: self.drop_constant,
        })
    }

    fn kind(&self) -> &'static str {
        RADIOMICS_QC_KIND
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
        if !(0.0..=1.0).contains(&self.max_missing) {
            return Err(DagError::Schedule(
                "max_feature_missing_rate must be between 0 and 1".into(),
            ));
        }
        let batches = collect_dataframe(inputs).await?;
        if batches.is_empty() {
            return Err(DagError::Schedule("radiomics_qc input is empty".into()));
        }
        let schema = batches[0].schema();
        let id_index = schema.index_of(&self.id_column).map_err(|_| {
            DagError::Schedule(format!(
                "radiomics_qc cannot find ID column `{}`",
                self.id_column
            ))
        })?;
        if !matches!(
            schema.field(id_index).data_type(),
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
        ) {
            return Err(DagError::Schedule(
                "radiomics_qc ID column must be a string".into(),
            ));
        }

        let mut ids = Vec::new();
        for batch in &batches {
            ids.extend(
                string_values(batch.column(id_index).as_ref())
                    .ok_or_else(|| {
                        DagError::Schedule("radiomics_qc ID column must be a string".into())
                    })?
                    .into_iter()
                    .map(|value| value.unwrap_or_default()),
            );
        }
        let mut seen = BTreeSet::new();
        for id in &ids {
            if !seen.insert(id.clone()) {
                return Err(DagError::Schedule(format!(
                    "radiomics_qc found duplicate ID `{id}`"
                )));
            }
        }

        let mut feature_indices = Vec::new();
        for (index, field) in schema.fields().iter().enumerate() {
            if index == id_index || RESERVED_FEATURE_SET_COLUMNS.contains(&field.name().as_str()) {
                continue;
            }
            if is_numeric_type(field.data_type()) {
                feature_indices.push(index);
            }
        }
        if feature_indices.is_empty() {
            return Err(DagError::Schedule(
                "radiomics_qc found no numeric feature columns".into(),
            ));
        }

        let mut feature_stats = BTreeMap::new();
        let mut row_missing = vec![0usize; ids.len()];
        let mut row_nonfinite = vec![0usize; ids.len()];
        for index in &feature_indices {
            let name = schema.field(*index).name().clone();
            let mut values = Vec::new();
            for batch in &batches {
                let cast = arrow::compute::cast(batch.column(*index), &DataType::Float64).map_err(
                    |error| {
                        DagError::Schedule(format!(
                            "cannot cast feature `{name}` to Float64: {error}"
                        ))
                    },
                )?;
                let array = cast
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| {
                        DagError::Schedule(format!("feature `{name}` did not cast to Float64"))
                    })?;
                values.extend((0..array.len()).map(|row| {
                    if array.is_null(row) {
                        None
                    } else {
                        Some(array.value(row))
                    }
                }));
            }
            let present = values.iter().filter_map(|value| *value).collect::<Vec<_>>();
            let missing = values.iter().filter(|value| value.is_none()).count();
            let nonfinite = present.iter().filter(|value| !value.is_finite()).count();
            let finite = present
                .iter()
                .copied()
                .filter(|value| value.is_finite())
                .collect::<Vec<_>>();
            let mean = if finite.is_empty() {
                f64::NAN
            } else {
                finite.iter().sum::<f64>() / finite.len() as f64
            };
            let variance = if finite.len() < 2 {
                f64::NAN
            } else {
                finite
                    .iter()
                    .map(|value| (value - mean).powi(2))
                    .sum::<f64>()
                    / (finite.len() - 1) as f64
            };
            let constant = finite.len() > 1 && variance == 0.0;
            let missing_rate = missing as f64 / values.len() as f64;
            let valid = nonfinite == 0
                && missing_rate <= self.max_missing
                && !(self.drop_constant && constant);
            feature_stats.insert(
                name.clone(),
                FeatureQcRow {
                    feature_id: name,
                    n_total: values.len() as i64,
                    n_missing: missing as i64,
                    n_nonfinite: nonfinite as i64,
                    missing_rate,
                    mean,
                    standard_deviation: variance.sqrt(),
                    minimum: finite.iter().copied().fold(f64::INFINITY, f64::min),
                    maximum: finite.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                    is_constant: constant,
                    status: if valid { "valid" } else { "invalid" }.into(),
                },
            );
            for (row, value) in values.iter().enumerate() {
                if value.is_none() {
                    row_missing[row] += 1;
                } else if !value.expect("checked missing").is_finite() {
                    row_nonfinite[row] += 1;
                }
            }
        }

        let valid_features = feature_stats
            .iter()
            .filter(|(_, stats)| stats.status == "valid")
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        if valid_features.is_empty() {
            return Err(DagError::Schedule(
                "radiomics_qc retained no features".into(),
            ));
        }

        let mut sample_rows = Vec::with_capacity(ids.len());
        let mut valid_row = Vec::with_capacity(ids.len());
        for row in 0..ids.len() {
            let missing_fraction = row_missing[row] as f64 / feature_indices.len() as f64;
            let valid = row_nonfinite[row] == 0 && missing_fraction <= self.max_missing;
            valid_row.push(valid);
            sample_rows.push(SampleQcRow {
                extraction_id: ids[row].clone(),
                n_features: feature_indices.len() as i32,
                n_missing: row_missing[row] as i32,
                n_nonfinite: row_nonfinite[row] as i32,
                missing_rate: missing_fraction,
                status: if valid { "valid" } else { "invalid" }.into(),
                reason: if valid {
                    String::new()
                } else if row_nonfinite[row] > 0 {
                    "feature values are not finite".into()
                } else {
                    "too many feature values are missing".into()
                },
            });
        }

        let valid_indices = valid_features
            .iter()
            .map(|name| schema.index_of(name))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                DagError::Schedule(format!("valid feature disappeared during QC: {error}"))
            })?;
        let mut filtered_fields = vec![schema.field(id_index).clone()];
        for index in &valid_indices {
            filtered_fields.push(schema.field(*index).clone());
        }
        let filtered_schema = Arc::new(Schema::new(filtered_fields));
        let mut filtered_batches = Vec::with_capacity(batches.len());
        let mut cursor = 0usize;
        for batch in &batches {
            let mut columns = vec![batch.column(id_index).clone()];
            columns.extend(
                valid_indices
                    .iter()
                    .map(|index| batch.column(*index).clone()),
            );
            let row_flags = valid_row[cursor..cursor + batch.num_rows()].to_vec();
            let filter = arrow_array::BooleanArray::from(row_flags);
            let mut filtered_columns = Vec::with_capacity(columns.len());
            for column in columns {
                filtered_columns.push(arrow::compute::filter(&column, &filter).map_err(
                    |error| DagError::Schedule(format!("cannot filter radiomics QC rows: {error}")),
                )?);
            }
            filtered_batches.push(
                RecordBatch::try_new(filtered_schema.clone(), filtered_columns).map_err(
                    |error| DagError::Schedule(format!("cannot construct QC output: {error}")),
                )?,
            );
            cursor += batch.num_rows();
        }

        let session = ctx.session();
        let feature_rows = feature_stats.into_values().collect::<Vec<_>>();
        let feature_df = session
            .read_batch(feature_qc_batch(&feature_rows).map_err(arrow_error)?)
            .map_err(|error| {
                DagError::Schedule(format!("cannot create feature QC table: {error}"))
            })?;
        let sample_df = session
            .read_batch(sample_qc_batch(&sample_rows).map_err(arrow_error)?)
            .map_err(|error| {
                DagError::Schedule(format!("cannot create sample QC table: {error}"))
            })?;
        let mut filtered_df: Option<DataFrame> = None;
        for batch in filtered_batches {
            let df = session.read_batch(batch).map_err(|error| {
                DagError::Schedule(format!("cannot create filtered feature table: {error}"))
            })?;
            filtered_df = Some(match filtered_df {
                Some(previous) => previous.union(df).map_err(|error| {
                    DagError::Schedule(format!("cannot union QC output batches: {error}"))
                })?,
                None => df,
            });
        }

        let mut outputs = PortOutputs::new();
        outputs.insert(0, sample_df);
        outputs.insert(1, feature_df);
        outputs.insert(2, filtered_df.expect("at least one QC batch"));
        Ok(outputs)
    }
}

pub struct RadiomicsQcNodeFactory;

impl NodeFactory for RadiomicsQcNodeFactory {
    fn kind(&self) -> &'static str {
        RADIOMICS_QC_KIND
    }

    fn desc(&self) -> &'static str {
        "Performs deterministic sample- and feature-level radiomics QC."
    }

    fn doc(&self) -> &'static str {
        "Checks a wide radiomics feature table for duplicate extraction IDs, \
        missing values, non-finite values, constant features, and per-row \
        missingness. Port 0 emits sample QC, port 1 feature QC, and port 2 a \
        filtered wide feature table. The node never trains a transformation \
        or infer statistics from an outcome variable."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RadiomicsQcSpec)
    }

    fn ports(&self) -> NodePorts {
        qc_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: RadiomicsQcSpec = serde_json::from_value(spec)?;
        if spec.id_column.trim().is_empty() {
            return Err(dag_core::registry::error::Error::Unknown(
                "id_column cannot be empty".into(),
            ));
        }
        if !(0.0..=1.0).contains(&spec.max_feature_missing_rate) {
            return Err(dag_core::registry::error::Error::Unknown(
                "max_feature_missing_rate must be between 0 and 1".into(),
            ));
        }
        Ok(Box::new(RadiomicsQcNode {
            ports: qc_ports(),
            id_column: spec.id_column,
            max_missing: spec.max_feature_missing_rate,
            drop_constant: spec.drop_constant_features,
        }))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RadiomicsFeatureSetAssembleSpec {
    /// Unique extraction-unit identifier column.
    #[serde(default = "default_extraction_column")]
    pub id_column: String,
    /// Stable identifier written to the assembled feature set.
    pub feature_set_id: String,
    /// Semantic version of the assembled feature set.
    #[serde(default = "default_feature_set_version")]
    pub feature_set_version: String,
}

fn default_feature_set_version() -> String {
    "v1".into()
}

pub struct RadiomicsFeatureSetAssembleNode {
    ports: NodePorts,
    id_column: String,
    feature_set_id: String,
    feature_set_version: String,
}

fn assemble_ports() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(None)
        .add_output_port(None)
}

#[async_trait]
impl DagNode for RadiomicsFeatureSetAssembleNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            id_column: self.id_column.clone(),
            feature_set_id: self.feature_set_id.clone(),
            feature_set_version: self.feature_set_version.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        RADIOMICS_FEATURE_SET_ASSEMBLE_KIND
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
        if self.feature_set_id.trim().is_empty() {
            return Err(DagError::Schedule("feature_set_id cannot be empty".into()));
        }
        let batches = collect_dataframe(inputs).await?;
        if batches.is_empty() {
            return Err(DagError::Schedule("feature set input is empty".into()));
        }
        let schema = batches[0].schema();
        let id_index = schema.index_of(&self.id_column).map_err(|_| {
            DagError::Schedule(format!("cannot find ID column `{}`", self.id_column))
        })?;
        let mut ids = Vec::new();
        for batch in &batches {
            ids.extend(
                string_values(batch.column(id_index).as_ref())
                    .ok_or_else(|| {
                        DagError::Schedule("feature set ID column must be a string".into())
                    })?
                    .into_iter()
                    .map(|value| value.unwrap_or_default()),
            );
        }
        let mut seen = BTreeSet::new();
        for id in &ids {
            if !seen.insert(id.clone()) {
                return Err(DagError::Schedule(format!(
                    "feature set has duplicate extraction ID `{id}`"
                )));
            }
        }

        let mut output_fields = Vec::new();
        let mut feature_fields = Vec::new();
        for field in schema.fields() {
            if field.name() == "feature_set_id" || field.name() == "feature_set_version" {
                continue;
            }
            output_fields.push(field.clone());
            if !RESERVED_FEATURE_SET_COLUMNS.contains(&field.name().as_str())
                && is_numeric_type(field.data_type())
            {
                feature_fields.push(field.clone());
            }
        }
        output_fields.push(Arc::new(Field::new(
            "feature_set_id",
            DataType::Utf8,
            false,
        )));
        output_fields.push(Arc::new(Field::new(
            "feature_set_version",
            DataType::Utf8,
            false,
        )));
        let output_schema = Arc::new(Schema::new(output_fields));

        let mut output_batches = Vec::with_capacity(batches.len());
        for batch in &batches {
            let mut columns = Vec::with_capacity(batch.num_columns());
            for (index, field) in schema.fields().iter().enumerate() {
                if field.name() == "feature_set_id" || field.name() == "feature_set_version" {
                    continue;
                }
                columns.push(batch.column(index).clone());
            }
            columns.push(Arc::new(StringArray::from(vec![
                self.feature_set_id.clone();
                batch.num_rows()
            ])));
            columns.push(Arc::new(StringArray::from(vec![
                self.feature_set_version
                    .clone();
                batch.num_rows()
            ])));
            output_batches.push(
                RecordBatch::try_new(output_schema.clone(), columns).map_err(|error| {
                    DagError::Schedule(format!("cannot assemble feature set: {error}"))
                })?,
            );
        }

        let metadata_rows = feature_fields
            .iter()
            .map(|field| FeatureMetadataRow {
                feature_id: field.name().clone(),
                pyradiomics_name: field.name().clone(),
                feature_family: infer_feature_family(field.name()),
                image_type: infer_image_type(field.name()),
                feature_set_id: self.feature_set_id.clone(),
                feature_set_version: self.feature_set_version.clone(),
            })
            .collect::<Vec<_>>();

        let session = ctx.session();
        let mut feature_set: Option<DataFrame> = None;
        for batch in output_batches {
            let df = session.read_batch(batch).map_err(|error| {
                DagError::Schedule(format!("cannot create assembled feature set: {error}"))
            })?;
            feature_set = Some(match feature_set {
                Some(previous) => previous.union(df).map_err(|error| {
                    DagError::Schedule(format!("cannot union assembled batches: {error}"))
                })?,
                None => df,
            });
        }
        let metadata_df = session
            .read_batch(feature_metadata_batch(&metadata_rows).map_err(arrow_error)?)
            .map_err(|error| {
                DagError::Schedule(format!("cannot create feature metadata: {error}"))
            })?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, feature_set.expect("input was nonempty"));
        outputs.insert(1, metadata_df);
        Ok(outputs)
    }
}

pub struct RadiomicsFeatureSetAssembleNodeFactory;

impl NodeFactory for RadiomicsFeatureSetAssembleNodeFactory {
    fn kind(&self) -> &'static str {
        RADIOMICS_FEATURE_SET_ASSEMBLE_KIND
    }

    fn desc(&self) -> &'static str {
        "Stamps a validated radiomics wide table as a versioned feature set."
    }

    fn doc(&self) -> &'static str {
        "Adds stable feature_set_id and feature_set_version columns while \
        rejecting duplicate extraction IDs. Port 0 emits the Stage-B wide \
        feature table and port 1 emits feature-level metadata derived from the \
        normalized feature IDs. Use SQL union_by_name before this node when \
        combining feature sets with different columns."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RadiomicsFeatureSetAssembleSpec)
    }

    fn ports(&self) -> NodePorts {
        assemble_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: RadiomicsFeatureSetAssembleSpec = serde_json::from_value(spec)?;
        if spec.id_column.trim().is_empty() || spec.feature_set_id.trim().is_empty() {
            return Err(dag_core::registry::error::Error::Unknown(
                "id_column and feature_set_id cannot be empty".into(),
            ));
        }
        Ok(Box::new(RadiomicsFeatureSetAssembleNode {
            ports: assemble_ports(),
            id_column: spec.id_column,
            feature_set_id: spec.feature_set_id,
            feature_set_version: spec.feature_set_version,
        }))
    }
}

#[derive(Default)]
struct ManifestRows {
    rows: Vec<ManifestRow>,
}

impl ManifestRows {
    fn push(&mut self, row: ManifestRow) {
        self.rows.push(row);
    }
}

#[derive(Clone)]
struct ManifestRow {
    extraction_id: String,
    patient_id: String,
    image_id: String,
    image_uri: String,
    mask_uri: String,
    modality: String,
    roi_id: String,
    roi_name: String,
    mask_label: i32,
    timepoint: String,
    preset_id: String,
    format: String,
    status: String,
    error_code: String,
}

#[derive(Clone, Default)]
struct SourceRow {
    extraction_id: Option<String>,
    patient_id: Option<String>,
    image_id: Option<String>,
    image: Option<String>,
    mask: Option<String>,
    modality: Option<String>,
    roi_id: Option<String>,
    roi_name: Option<String>,
    mask_label: Option<f64>,
    timepoint: Option<String>,
    preset_id: Option<String>,
}

#[derive(Clone)]
struct SourceColumns {
    extraction_id: Option<String>,
    patient_id: String,
    image_id: String,
    image: String,
    mask: String,
    modality: String,
    roi_id: String,
    roi_name: Option<String>,
    mask_label: String,
    timepoint: Option<String>,
    preset_id: String,
}

impl SourceColumns {
    fn from_manifest_spec(spec: &RadiomicsManifestSpec) -> Self {
        Self {
            extraction_id: spec.extraction_id_column.clone(),
            patient_id: spec.patient_id_column.clone(),
            image_id: spec.image_id_column.clone(),
            image: spec.image_column.clone(),
            mask: spec.mask_column.clone(),
            modality: spec.modality_column.clone(),
            roi_id: spec.roi_id_column.clone(),
            roi_name: spec.roi_name_column.clone(),
            mask_label: spec.mask_label_column.clone(),
            timepoint: spec.timepoint_column.clone(),
            preset_id: spec.preset_id_column.clone(),
        }
    }
}

struct DiagnosticRow {
    extraction_id: String,
    row: i32,
    severity: String,
    error_code: String,
    message: String,
}

struct SampleQcRow {
    extraction_id: String,
    n_features: i32,
    n_missing: i32,
    n_nonfinite: i32,
    missing_rate: f64,
    status: String,
    reason: String,
}

struct FeatureQcRow {
    feature_id: String,
    n_total: i64,
    n_missing: i64,
    n_nonfinite: i64,
    missing_rate: f64,
    mean: f64,
    standard_deviation: f64,
    minimum: f64,
    maximum: f64,
    is_constant: bool,
    status: String,
}

struct FeatureMetadataRow {
    feature_id: String,
    pyradiomics_name: String,
    feature_family: String,
    image_type: String,
    feature_set_id: String,
    feature_set_version: String,
}

async fn collect_dataframe(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or_else(|| {
        DagError::Schedule("radiomics node has no connected DataFrame input".into())
    })?;
    let df = input.dataframe()?;
    let batches =
        df.clone().collect().await.map_err(|error| {
            DagError::Schedule(format!("cannot collect radiomics input: {error}"))
        })?;
    if batches.iter().any(|batch| batch.num_rows() == 0) && batches.len() == 1 {
        return Err(DagError::Schedule("radiomics input has no rows".into()));
    }
    Ok(batches)
}

fn extract_manifest_rows(
    batches: &[RecordBatch],
    columns: &SourceColumns,
) -> Result<Vec<SourceRow>, DagError> {
    if batches.is_empty() || batches.iter().map(RecordBatch::num_rows).sum::<usize>() == 0 {
        return Err(DagError::Schedule("manifest input has no rows".into()));
    }
    let schema = batches[0].schema();
    let mut mappings = BTreeMap::new();
    if let Some(column) = &columns.extraction_id {
        mappings.insert(column.clone(), 0);
    }
    mappings.insert(columns.patient_id.clone(), 1);
    mappings.insert(columns.image_id.clone(), 2);
    mappings.insert(columns.image.clone(), 3);
    mappings.insert(columns.mask.clone(), 4);
    mappings.insert(columns.modality.clone(), 5);
    mappings.insert(columns.roi_id.clone(), 6);
    if let Some(column) = &columns.roi_name {
        mappings.insert(column.clone(), 7);
    }
    mappings.insert(columns.mask_label.clone(), 8);
    if let Some(column) = &columns.timepoint {
        mappings.insert(column.clone(), 9);
    }
    mappings.insert(columns.preset_id.clone(), 10);
    let mut indices = BTreeMap::new();
    for column in mappings.keys() {
        let index = schema.index_of(column).map_err(|_| {
            DagError::Schedule(format!("radiomics manifest cannot find column `{column}`"))
        })?;
        indices.insert(column.clone(), index);
    }

    let mut rows = Vec::new();
    for batch in batches {
        let mut values = BTreeMap::new();
        for (column, index) in &indices {
            values.insert(
                column.as_str(),
                string_values(batch.column(*index).as_ref()),
            );
        }
        let mask_label_values = numeric_values(
            batch
                .column(*indices.get(&columns.mask_label).expect("mapped"))
                .as_ref(),
        );
        for row in 0..batch.num_rows() {
            let get = |column: &str| -> Option<String> {
                values
                    .get(column)?
                    .as_ref()?
                    .get(row)
                    .and_then(Clone::clone)
            };
            let mask_label = mask_label_values
                .as_ref()
                .and_then(|values| values.get(row).copied().flatten())
                .or_else(|| get(&columns.mask_label).and_then(|value| value.parse::<f64>().ok()));
            rows.push(SourceRow {
                extraction_id: columns.extraction_id.as_deref().and_then(&get),
                patient_id: get(&columns.patient_id),
                image_id: get(&columns.image_id),
                image: get(&columns.image),
                mask: get(&columns.mask),
                modality: get(&columns.modality),
                roi_id: get(&columns.roi_id),
                roi_name: columns.roi_name.as_deref().and_then(&get),
                mask_label,
                timepoint: columns.timepoint.as_deref().and_then(&get),
                preset_id: get(&columns.preset_id),
            });
        }
    }
    Ok(rows)
}

fn required_or_invalid(value: &Option<String>, name: &str, errors: &mut Vec<String>) -> String {
    value
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            errors.push(format!("{name} is missing or empty"));
            format!("__missing_{}__", name.to_lowercase())
        })
}

fn string_values(array: &dyn Array) -> Option<Vec<Option<String>>> {
    use arrow_array::{LargeStringArray, StringArray, StringViewArray};
    if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
        return Some(
            (0..values.len())
                .map(|index| (!values.is_null(index)).then(|| values.value(index).to_string()))
                .collect(),
        );
    }
    if let Some(values) = array.as_any().downcast_ref::<LargeStringArray>() {
        return Some(
            (0..values.len())
                .map(|index| (!values.is_null(index)).then(|| values.value(index).to_string()))
                .collect(),
        );
    }
    array
        .as_any()
        .downcast_ref::<StringViewArray>()
        .map(|values| {
            (0..values.len())
                .map(|index| (!values.is_null(index)).then(|| values.value(index).to_string()))
                .collect()
        })
}

fn numeric_values(array: &dyn Array) -> Option<Vec<Option<f64>>> {
    use arrow_array::{
        Float32Array, Int8Array, Int16Array, Int32Array, Int64Array, UInt8Array, UInt16Array,
        UInt32Array, UInt64Array,
    };
    macro_rules! numeric_values {
        ($array:expr, $array_type:ty) => {
            $array.as_any().downcast_ref::<$array_type>().map(|values| {
                (0..values.len())
                    .map(|index| (!values.is_null(index)).then(|| values.value(index) as f64))
                    .collect::<Vec<_>>()
            })
        };
    }
    numeric_values!(array, Int8Array)
        .or_else(|| numeric_values!(array, Int16Array))
        .or_else(|| numeric_values!(array, Int32Array))
        .or_else(|| numeric_values!(array, Int64Array))
        .or_else(|| numeric_values!(array, UInt8Array))
        .or_else(|| numeric_values!(array, UInt16Array))
        .or_else(|| numeric_values!(array, UInt32Array))
        .or_else(|| numeric_values!(array, UInt64Array))
        .or_else(|| numeric_values!(array, Float32Array))
        .or_else(|| numeric_values!(array, Float64Array))
}

fn is_numeric_type(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float32
            | DataType::Float64
    )
}

fn infer_format(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".nii.gz") {
        "nifti".into()
    } else if lower.ends_with(".nii") {
        "nifti".into()
    } else if lower.ends_with(".mha") || lower.ends_with(".mhd") {
        "mha".into()
    } else if lower.ends_with(".dcm") || lower.ends_with(".dicom") {
        "dicom".into()
    } else {
        "unknown".into()
    }
}

fn derive_extraction_id(patient_id: &str, image_id: &str, roi_id: &str) -> String {
    let raw = format!("{patient_id}|{image_id}|{roi_id}");
    let digest = Sha256::digest(raw.as_bytes());
    let suffix = digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{patient_id}_{image_id}_{roi_id}_{suffix}")
}

fn arrow_error(error: arrow_schema::ArrowError) -> DagError {
    DagError::Schedule(format!("cannot construct Arrow table: {error}"))
}

fn manifest_record_batch(rows: &ManifestRows) -> Result<RecordBatch, arrow_schema::ArrowError> {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("extraction_id", DataType::Utf8, false),
            Field::new("patient_id", DataType::Utf8, false),
            Field::new("image_id", DataType::Utf8, false),
            Field::new("image_uri", DataType::Utf8, false),
            Field::new("mask_uri", DataType::Utf8, false),
            Field::new("modality", DataType::Utf8, false),
            Field::new("roi_id", DataType::Utf8, false),
            Field::new("roi_name", DataType::Utf8, true),
            Field::new("mask_label", DataType::Int32, false),
            Field::new("timepoint", DataType::Utf8, true),
            Field::new("preset_id", DataType::Utf8, false),
            Field::new("format", DataType::Utf8, false),
            Field::new("status", DataType::Utf8, false),
            Field::new("error_code", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.extraction_id.clone())
                    .collect::<Vec<_>>(),
            )) as ArrayRef,
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.patient_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.image_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.image_uri.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.mask_uri.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.modality.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.roi_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.roi_name.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int32Array::from(
                rows.rows
                    .iter()
                    .map(|row| row.mask_label)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.timepoint.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.preset_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.format.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.status.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.rows
                    .iter()
                    .map(|row| row.error_code.clone())
                    .collect::<Vec<_>>(),
            )),
        ],
    )
}

fn diagnostics_record_batch(
    rows: &[DiagnosticRow],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("extraction_id", DataType::Utf8, false),
            Field::new("row", DataType::Int32, false),
            Field::new("severity", DataType::Utf8, false),
            Field::new("error_code", DataType::Utf8, false),
            Field::new("message", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.extraction_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int32Array::from(
                rows.iter().map(|row| row.row).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.severity.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.error_code.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.message.clone())
                    .collect::<Vec<_>>(),
            )),
        ],
    )
}

fn sample_qc_batch(rows: &[SampleQcRow]) -> Result<RecordBatch, arrow_schema::ArrowError> {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("extraction_id", DataType::Utf8, false),
            Field::new("n_features", DataType::Int32, false),
            Field::new("n_missing", DataType::Int32, false),
            Field::new("n_nonfinite", DataType::Int32, false),
            Field::new("missing_rate", DataType::Float64, false),
            Field::new("status", DataType::Utf8, false),
            Field::new("reason", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.extraction_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int32Array::from(
                rows.iter().map(|row| row.n_features).collect::<Vec<_>>(),
            )),
            Arc::new(Int32Array::from(
                rows.iter().map(|row| row.n_missing).collect::<Vec<_>>(),
            )),
            Arc::new(Int32Array::from(
                rows.iter().map(|row| row.n_nonfinite).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.missing_rate).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.status.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.reason.clone())
                    .collect::<Vec<_>>(),
            )),
        ],
    )
}

fn feature_qc_batch(rows: &[FeatureQcRow]) -> Result<RecordBatch, arrow_schema::ArrowError> {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("feature_id", DataType::Utf8, false),
            Field::new("n_total", DataType::Int64, false),
            Field::new("n_missing", DataType::Int64, false),
            Field::new("n_nonfinite", DataType::Int64, false),
            Field::new("missing_rate", DataType::Float64, false),
            Field::new("mean", DataType::Float64, true),
            Field::new("standard_deviation", DataType::Float64, true),
            Field::new("minimum", DataType::Float64, true),
            Field::new("maximum", DataType::Float64, true),
            Field::new("is_constant", DataType::Boolean, false),
            Field::new("status", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.feature_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                rows.iter().map(|row| row.n_total).collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                rows.iter().map(|row| row.n_missing).collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                rows.iter().map(|row| row.n_nonfinite).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.missing_rate).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.mean).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|row| row.standard_deviation)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.minimum).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.maximum).collect::<Vec<_>>(),
            )),
            Arc::new(arrow_array::BooleanArray::from(
                rows.iter().map(|row| row.is_constant).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.status.clone())
                    .collect::<Vec<_>>(),
            )),
        ],
    )
}

fn feature_metadata_batch(
    rows: &[FeatureMetadataRow],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("feature_id", DataType::Utf8, false),
            Field::new("pyradiomics_name", DataType::Utf8, false),
            Field::new("feature_family", DataType::Utf8, false),
            Field::new("image_type", DataType::Utf8, false),
            Field::new("feature_set_id", DataType::Utf8, false),
            Field::new("feature_set_version", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.feature_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.pyradiomics_name.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.feature_family.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.image_type.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.feature_set_id.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.feature_set_version.clone())
                    .collect::<Vec<_>>(),
            )),
        ],
    )
}

fn infer_feature_family(feature_id: &str) -> String {
    let lower = feature_id.to_ascii_lowercase();
    for family in [
        "shape2d",
        "shape",
        "firstorder",
        "glcm",
        "glrlm",
        "glszm",
        "gldm",
        "ngtdm",
    ] {
        if lower.contains(&format!("_{family}_"))
            || lower.ends_with(family)
            || lower.starts_with(family)
        {
            return family.to_string();
        }
    }
    "other".into()
}

fn infer_image_type(feature_id: &str) -> String {
    let lower = feature_id.to_ascii_lowercase();
    if lower.starts_with("original_") {
        return "original".into();
    }
    if lower.starts_with("wavelet_") {
        return "wavelet".into();
    }
    if lower.starts_with("log_") {
        return "log".into();
    }
    if lower.starts_with("square_") {
        return "square".into();
    }
    if lower.starts_with("squareroot_") {
        return "squareroot".into();
    }
    if lower.starts_with("logarithm_") {
        return "logarithm".into();
    }
    if lower.starts_with("exponential_") {
        return "exponential".into();
    }
    if lower.starts_with("gradient_") {
        return "gradient".into();
    }
    if lower.starts_with("lbp_") {
        return "lbp".into();
    }
    "other".into()
}

async fn resolve_file_ref(ctx: &NodeCtx, path: &str) -> Result<FileRef, String> {
    if path.trim().is_empty() {
        return Err("path is empty".into());
    }
    if let Some(storage) = ctx.opendal.as_ref() {
        let virtual_path = path
            .strip_prefix("vfs://")
            .map(str::to_string)
            .or_else(|| path.strip_prefix("file://").map(str::to_string))
            .or_else(|| path.starts_with('/').then(|| path.to_string()));
        if let Some(virtual_path) = virtual_path {
            let normalized = vfs::OpendalFileStorage::normalize_path(&virtual_path);
            if storage.is_mounted(&normalized) {
                let operator = storage.resolve(&normalized);
                let key = storage.resolve_path(&normalized);
                let metadata = operator
                    .stat(&key)
                    .await
                    .map_err(|error| error.to_string())?;
                if metadata.is_dir() {
                    return Err("path is a directory".into());
                }
                let mtime_ns = metadata
                    .last_modified()
                    .and_then(|timestamp| {
                        std::time::SystemTime::from(timestamp)
                            .duration_since(std::time::UNIX_EPOCH)
                            .ok()
                    })
                    .map(|duration| duration.as_nanos() as i128)
                    .unwrap_or_default();
                return Ok(FileRef {
                    path: path.to_string(),
                    format: None,
                    fingerprint: Some(FileFingerprint {
                        size: metadata.content_length(),
                        mtime_ns: mtime_ns as i128,
                        content_hash: metadata.etag().map(str::to_string),
                        // The original path may be a bare mounted virtual
                        // path — immutable remote regardless of spelling.
                        immutable_remote: true,
                    }),
                });
            }
        }
    }
    let local = StdPath::new(path.strip_prefix("file://").unwrap_or(path));
    if !local.is_absolute() {
        return Err("path must be absolute, file://, or vfs://".into());
    }
    let metadata = std::fs::metadata(local).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("path is not a regular file".into());
    }
    Ok(FileRef {
        path: path.to_string(),
        format: None,
        fingerprint: Some(FileFingerprint::from_metadata(&metadata)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Int32Array;

    fn ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    fn input_batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("patient_id", DataType::Utf8, false),
                Field::new("image_id", DataType::Utf8, false),
                Field::new("image_uri", DataType::Utf8, false),
                Field::new("mask_uri", DataType::Utf8, false),
                Field::new("modality", DataType::Utf8, false),
                Field::new("roi_id", DataType::Utf8, false),
                Field::new("mask_label", DataType::Int32, false),
                Field::new("preset_id", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["STS_001", "STS_001"])),
                Arc::new(StringArray::from(vec!["CT", "PET"])),
                Arc::new(StringArray::from(vec![
                    "/input/ct.nii.gz",
                    "/input/pet.nii.gz",
                ])),
                Arc::new(StringArray::from(vec![
                    "/input/ct.mask.nii.gz",
                    "/input/pet.mask.nii.gz",
                ])),
                Arc::new(StringArray::from(vec!["ct", "pet"])),
                Arc::new(StringArray::from(vec!["GTV_Mass", "GTV_Mass"])),
                Arc::new(Int32Array::from(vec![1, 1])),
                Arc::new(StringArray::from(vec![
                    "pyradiomics_original_v1",
                    "pyradiomics_original_v1",
                ])),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn dcm_glob_returns_sorted_local_file_set() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("000002.dcm"), b"two").unwrap();
        std::fs::write(directory.path().join("000001.dcm"), b"one").unwrap();
        std::fs::write(directory.path().join("not-dicom.txt"), b"ignored").unwrap();
        let pattern = directory
            .path()
            .join("*.dcm")
            .to_string_lossy()
            .into_owned();
        let node_ctx = ctx();
        let mut node = RadiomicsDcmGlobNodeFactory
            .build(serde_json::json!({"pattern": pattern}), node_ctx.clone())
            .unwrap();
        let outputs = node
            .execute(
                &node_ctx,
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let files = outputs.get(&0).unwrap().as_file_set().unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].path.ends_with("000001.dcm"));
        assert!(files[1].path.ends_with("000002.dcm"));
        assert_eq!(files[0].format.as_deref(), Some("dicom"));
    }

    #[tokio::test]
    async fn manifest_normalizes_without_path_validation() {
        let node_ctx = ctx();
        let df = node_ctx.session().read_batch(input_batch()).unwrap();
        let mut node = RadiomicsManifestNode {
            ports: manifest_port_layout(),
            spec: RadiomicsManifestSpec {
                image_column: default_image_column(),
                mask_column: default_mask_column(),
                patient_id_column: default_patient_column(),
                image_id_column: default_image_id_column(),
                extraction_id_column: None,
                modality_column: default_modality_column(),
                roi_id_column: default_roi_column(),
                roi_name_column: None,
                mask_label_column: default_mask_label_column(),
                timepoint_column: None,
                preset_id_column: default_preset_column(),
                validate_paths: false,
            },
        };
        let outputs = node
            .execute(
                &node_ctx,
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let manifest = outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(manifest[0].num_rows(), 2);
        assert_eq!(manifest[0].column_by_name("modality").unwrap().len(), 2);
    }

    #[tokio::test]
    async fn qc_filters_constant_and_nonfinite_features() {
        let node_ctx = ctx();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("extraction_id", DataType::Utf8, false),
                Field::new("original_firstorder_mean", DataType::Float64, false),
                Field::new("original_shape_constant", DataType::Float64, false),
                Field::new("original_firstorder_maximum", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["a", "b"])),
                Arc::new(Float64Array::from(vec![1.0, f64::NAN])),
                Arc::new(Float64Array::from(vec![2.0, 2.0])),
                Arc::new(Float64Array::from(vec![3.0, 5.0])),
            ],
        )
        .unwrap();
        let df = node_ctx.session().read_batch(batch).unwrap();
        let mut node = RadiomicsQcNode {
            ports: qc_ports(),
            id_column: default_extraction_column(),
            max_missing: 0.5,
            drop_constant: true,
        };
        let outputs = node
            .execute(
                &node_ctx,
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let samples = outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        let features = outputs
            .dataframe(1)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(samples[0].num_rows(), 2);
        assert!(features[0].num_rows() >= 2);
    }
}
