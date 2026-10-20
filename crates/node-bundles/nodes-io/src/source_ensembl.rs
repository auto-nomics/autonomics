//! Ensembl REST source nodes.
//!
//! These zero-input nodes wrap the typed SDK client and emit runtime-schema
//! DataFrames. Keeping them as sources avoids embedding identifiers or regions
//! in graph edges and makes Ensembl annotations composable downstream.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use ensembl::{EnsemblClient, SequenceType, types::*};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use dag_core::dag::{DagError, DagNode, NodeInput, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

fn client(endpoint: Option<&str>) -> EnsemblClient {
    match endpoint {
        Some(endpoint) => EnsemblClient::new().with_endpoint(endpoint),
        None => EnsemblClient::new(),
    }
}

fn output_port() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

fn err(operation: &str, error: ensembl::EnsemblError) -> DagError {
    DagError::Schedule(format!("{operation} failed: {error}"))
}

fn invalid(operation: &str, message: impl Into<String>) -> DagError {
    DagError::Schedule(format!("{operation}: {}", message.into()))
}

async fn emit(
    ctx: &NodeCtx,
    operation: &str,
    table: ensembl::convert::Table,
) -> Result<PortOutputs, DagError> {
    let batch = table_to_batch(operation, &table)?;
    let dataframe = ctx.session().read_batch(batch).map_err(|error| {
        DagError::Schedule(format!("failed to read {operation} batch: {error}"))
    })?;
    let mut outputs = PortOutputs::new();
    outputs.insert(0, dataframe);
    Ok(outputs)
}

fn table_to_batch(
    operation: &str,
    table: &ensembl::convert::Table,
) -> Result<RecordBatch, DagError> {
    if table.columns.is_empty() {
        let schema = Schema::new(vec![Field::new("response", DataType::Utf8, true)]);
        return RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(StringArray::from(Vec::<Option<&str>>::new()))],
        )
        .map_err(|error| {
            DagError::Schedule(format!("failed to build {operation} batch: {error}"))
        });
    }

    let fields = table
        .columns
        .iter()
        .map(|name| Field::new(name.clone(), DataType::Utf8, true))
        .collect::<Vec<_>>();
    let arrays = table
        .columns
        .iter()
        .enumerate()
        .map(|(column, _)| {
            let values = table
                .rows
                .iter()
                .map(|row| row.get(column).and_then(value_to_string))
                .collect::<Vec<_>>();
            let values = values
                .iter()
                .map(|value| value.as_deref())
                .collect::<Vec<_>>();
            Arc::new(StringArray::from(values)) as Arc<dyn Array>
        })
        .collect::<Vec<_>>();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
        .map_err(|error| DagError::Schedule(format!("failed to build {operation} batch: {error}")))
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        other => Some(other.to_string()),
    }
}

/// Serialize typed rows through the SDK's deterministic table normalization.
fn table<T: Serialize>(operation: &str, rows: &[T]) -> Result<ensembl::convert::Table, DagError> {
    let values = rows
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| invalid(operation, format!("encode response: {error}")))?;
    ensembl::convert::Table::from_json_rows(&values)
        .map_err(|error| invalid(operation, format!("normalize response: {error}")))
}

fn strings(values: Vec<Option<String>>) -> Arc<dyn Array> {
    let values = values
        .iter()
        .map(|value| value.as_deref())
        .collect::<Vec<_>>();
    Arc::new(StringArray::from(values))
}

fn nullable_strings<T: Into<Option<String>>>(rows: Vec<T>) -> Arc<dyn Array> {
    strings(rows.into_iter().map(|value| value.into()).collect())
}

fn opt_u32<T: Into<Option<u32>>>(rows: Vec<T>) -> Arc<dyn Array> {
    Arc::new(UInt32Array::from(
        rows.into_iter()
            .map(|value| value.into())
            .collect::<Vec<_>>(),
    ))
}

fn opt_f64<T: Into<Option<f64>>>(rows: Vec<T>) -> Arc<dyn Array> {
    Arc::new(Float64Array::from(
        rows.into_iter()
            .map(|value| value.into())
            .collect::<Vec<_>>(),
    ))
}

fn batch(fields: Vec<Field>, arrays: Vec<Arc<dyn Array>>) -> Result<RecordBatch, DagError> {
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
        .map_err(|error| DagError::Schedule(format!("failed to build Ensembl batch: {error}")))
}

fn dataframe(ctx: &NodeCtx, operation: &str, value: RecordBatch) -> Result<DataFrame, DagError> {
    ctx.session()
        .read_batch(value)
        .map_err(|error| DagError::Schedule(format!("failed to read {operation} batch: {error}")))
}

/// Spec for `source_ensembl_species`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EnsemblSpeciesSpec {
    /// Optional case-insensitive filter over scientific name, common name,
    /// alias, taxonomy ID, assembly, or division.
    #[serde(default)]
    pub query: Option<String>,
    /// Exact Ensembl division, e.g. `EnsemblVertebrates`.
    #[serde(default)]
    pub division: Option<String>,
    /// Endpoint override for tests and private Ensembl mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Ensembl `/info/species` source.
#[derive(Clone)]
pub struct EnsemblSpeciesNode {
    meta: NodePorts,
    spec: EnsemblSpeciesSpec,
}

pub struct EnsemblSpeciesNodeFactory;

impl NodeFactory for EnsemblSpeciesNodeFactory {
    fn kind(&self) -> &'static str {
        "source_ensembl_species"
    }

    fn desc(&self) -> &'static str {
        "Fetch Ensembl species metadata as a DataFrame."
    }

    fn doc(&self) -> &'static str {
        "Calls `/info/species` and optionally filters by division or a \
         case-insensitive substring. Runtime schema: `name, display_name, \
         common_name, division, release, assembly, accession, taxon_id, \
         aliases, groups`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnsemblSpeciesSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EnsemblSpeciesSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EnsemblSpeciesNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EnsemblSpeciesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_ensembl_species"
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
        let operation = "Ensembl species";
        let response = client(self.spec.endpoint.as_deref())
            .species()
            .await
            .map_err(|error| err(operation, error))?;
        let mut species = response.species;
        if let Some(division) = &self.spec.division {
            species.retain(|item| item.division.eq_ignore_ascii_case(division));
        }
        if let Some(query) = &self.spec.query {
            let query = query.to_ascii_lowercase();
            species.retain(|item| {
                let aliases = item
                    .aliases
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(&query));
                let fields = [
                    Some(item.name.to_ascii_lowercase()),
                    item.display_name.as_deref().map(str::to_ascii_lowercase),
                    item.common_name.as_deref().map(str::to_ascii_lowercase),
                    Some(item.taxon_id.to_ascii_lowercase()),
                    item.assembly.as_deref().map(str::to_ascii_lowercase),
                    Some(item.division.to_ascii_lowercase()),
                ];
                aliases
                    || fields
                        .into_iter()
                        .flatten()
                        .any(|value| value.contains(&query))
            });
        }

        let fields = vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("display_name", DataType::Utf8, true),
            Field::new("common_name", DataType::Utf8, true),
            Field::new("division", DataType::Utf8, false),
            Field::new("release", DataType::UInt32, false),
            Field::new("assembly", DataType::Utf8, true),
            Field::new("accession", DataType::Utf8, true),
            Field::new("taxon_id", DataType::Utf8, false),
            Field::new("aliases", DataType::Utf8, true),
            Field::new("groups", DataType::Utf8, true),
        ];
        let arrays = vec![
            strings(species.iter().map(|item| Some(item.name.clone())).collect()),
            nullable_strings(
                species
                    .iter()
                    .map(|item| item.display_name.clone())
                    .collect(),
            ),
            nullable_strings(
                species
                    .iter()
                    .map(|item| item.common_name.clone())
                    .collect(),
            ),
            strings(
                species
                    .iter()
                    .map(|item| Some(item.division.clone()))
                    .collect(),
            ),
            Arc::new(UInt32Array::from(
                species
                    .iter()
                    .map(|item| Some(item.release))
                    .collect::<Vec<_>>(),
            )),
            nullable_strings(species.iter().map(|item| item.assembly.clone()).collect()),
            nullable_strings(species.iter().map(|item| item.accession.clone()).collect()),
            strings(
                species
                    .iter()
                    .map(|item| Some(item.taxon_id.clone()))
                    .collect(),
            ),
            strings(
                species
                    .iter()
                    .map(|item| Some(item.aliases.join(", ")).filter(|v| !v.is_empty()))
                    .collect(),
            ),
            strings(
                species
                    .iter()
                    .map(|item| Some(item.groups.join(", ")).filter(|v| !v.is_empty()))
                    .collect(),
            ),
        ];
        let value = batch(fields, arrays)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe(ctx, operation, value)?);
        Ok(outputs)
    }
}

/// Spec for `source_ensembl_assembly`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EnsemblAssemblySpec {
    /// Ensembl species name, alias, common name, or taxonomy ID.
    pub species: String,
    /// Endpoint override for tests and private Ensembl mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Ensembl `/info/assembly/{species}` source.
#[derive(Clone)]
pub struct EnsemblAssemblyNode {
    meta: NodePorts,
    spec: EnsemblAssemblySpec,
}

pub struct EnsemblAssemblyNodeFactory;

impl NodeFactory for EnsemblAssemblyNodeFactory {
    fn kind(&self) -> &'static str {
        "source_ensembl_assembly"
    }

    fn desc(&self) -> &'static str {
        "Fetch assembly and coordinate-system metadata for a species."
    }

    fn doc(&self) -> &'static str {
        "Calls `/info/assembly/{species}` and emits one row with assembly \
         identity, genebuild dates, golden-path length, karyotype regions, and \
         top-level coordinate regions."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnsemblAssemblySpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EnsemblAssemblySpec = serde_json::from_value(spec)?;
        Ok(Box::new(EnsemblAssemblyNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EnsemblAssemblyNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_ensembl_assembly"
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
        let operation = "Ensembl assembly";
        let assembly = client(self.spec.endpoint.as_deref())
            .assembly(&self.spec.species)
            .await
            .map_err(|error| err(operation, error))?;
        emit(ctx, operation, table(operation, &[assembly])?).await
    }
}

/// Spec for `source_ensembl_lookup`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EnsemblLookupSpec {
    /// One or more Ensembl stable IDs. The endpoint accepts up to 1000.
    pub ids: Vec<String>,
    /// Expand transcript, translation, and exon records for genes.
    #[serde(default)]
    pub expand: bool,
    /// Optional species used when a stable ID is ambiguous.
    #[serde(default)]
    pub species: Option<String>,
    /// Endpoint override for tests and private Ensembl mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Ensembl stable-ID lookup source.
#[derive(Clone)]
pub struct EnsemblLookupNode {
    meta: NodePorts,
    spec: EnsemblLookupSpec,
}

pub struct EnsemblLookupNodeFactory;

impl NodeFactory for EnsemblLookupNodeFactory {
    fn kind(&self) -> &'static str {
        "source_ensembl_lookup"
    }

    fn desc(&self) -> &'static str {
        "Resolve Ensembl stable IDs to typed feature records."
    }

    fn doc(&self) -> &'static str {
        "Calls `/lookup/id` (batch POST when multiple IDs are supplied) and \
         emits one row per feature. With `expand=true`, nested transcripts, \
         translations, and exons are retained as JSON strings."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnsemblLookupSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EnsemblLookupSpec = serde_json::from_value(spec)?;
        if spec.ids.is_empty() {
            return Err(dag_core::registry::error::Error::Unknown(
                "source_ensembl_lookup requires at least one ID".into(),
            ));
        }
        Ok(Box::new(EnsemblLookupNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EnsemblLookupNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_ensembl_lookup"
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
        let operation = "Ensembl lookup";
        let client = client(self.spec.endpoint.as_deref());
        let entries = if self.spec.ids.len() == 1 {
            let id = self.spec.ids[0].clone();
            let entry = match self.spec.species.as_deref() {
                Some(species) => {
                    client
                        .lookup_id_with_species(species, &id, self.spec.expand)
                        .await
                }
                None => client.lookup_id(&id, self.spec.expand).await,
            };
            vec![entry.map_err(|error| err(operation, error))?]
        } else {
            let raw: HashMap<String, Value> = client
                .lookup_ids(&self.spec.ids, self.spec.expand)
                .await
                .map_err(|error| err(operation, error))?;
            // Preserve the request order. The endpoint normally returns every
            // requested key; a missing key is represented as a JSON null and
            // surfaces as a decoding error rather than silently changing rows.
            self.spec
                .ids
                .iter()
                .map(|id| {
                    let value = raw.get(id).cloned().unwrap_or(Value::Null);
                    serde_json::from_value::<LookupEntry>(value)
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| invalid(operation, format!("decode response: {error}")))?
        };
        emit(
            ctx,
            operation,
            ensembl::convert::lookup_table(&entries)
                .map_err(|error| invalid(operation, format!("normalize response: {error}")))?,
        )
        .await
    }
}

/// Spec for `source_ensembl_xrefs`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EnsemblXrefsSpec {
    /// Ensembl gene, transcript, translation, exon, or regulatory feature ID.
    pub id: String,
    /// Include dependent transcript and protein cross-references.
    #[serde(default = "default_true")]
    pub all_levels: bool,
    /// Optional external database filter, e.g. `HGNC`, `EntrezGene`, `UniProtKB`.
    #[serde(default)]
    pub external_db: Option<String>,
    /// Endpoint override for tests and private Ensembl mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Ensembl cross-reference source.
#[derive(Clone)]
pub struct EnsemblXrefsNode {
    meta: NodePorts,
    spec: EnsemblXrefsSpec,
}

pub struct EnsemblXrefsNodeFactory;

impl NodeFactory for EnsemblXrefsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_ensembl_xrefs"
    }

    fn desc(&self) -> &'static str {
        "Fetch external database cross-references for an Ensembl stable ID."
    }

    fn doc(&self) -> &'static str {
        "Calls `/xrefs/id/{id}` and emits one row per cross-reference. Output \
         includes database name, display identifier, primary ID, description, \
         info fields, version, and synonyms."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnsemblXrefsSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EnsemblXrefsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EnsemblXrefsNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EnsemblXrefsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_ensembl_xrefs"
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
        let operation = "Ensembl xrefs";
        let xrefs = client(self.spec.endpoint.as_deref())
            .xrefs(
                &self.spec.id,
                self.spec.all_levels,
                self.spec.external_db.as_deref(),
            )
            .await
            .map_err(|error| err(operation, error))?;
        emit(
            ctx,
            operation,
            ensembl::convert::xrefs_table(&xrefs)
                .map_err(|error| invalid(operation, format!("normalize response: {error}")))?,
        )
        .await
    }
}

/// Spec for `source_ensembl_overlap`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EnsemblOverlapSpec {
    /// Species name, alias, common name, or taxonomy ID.
    pub species: String,
    /// Region in `chromosome:start-end` form; an optional `:strand` suffix.
    pub region: String,
    /// Feature types, e.g. `gene`, `transcript`, `exon`, `cds`, `regulatory_feature`, `motif`.
    pub features: Vec<String>,
    /// Endpoint override for tests and private Ensembl mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Ensembl region overlap source.
#[derive(Clone)]
pub struct EnsemblOverlapNode {
    meta: NodePorts,
    spec: EnsemblOverlapSpec,
}

pub struct EnsemblOverlapNodeFactory;

impl NodeFactory for EnsemblOverlapNodeFactory {
    fn kind(&self) -> &'static str {
        "source_ensembl_overlap"
    }

    fn desc(&self) -> &'static str {
        "Fetch genomic features overlapping a region."
    }

    fn doc(&self) -> &'static str {
        "Calls `/overlap/region/{species}/{region}` and emits one row per \
         requested feature. The runtime schema follows the feature types \
         returned by Ensembl."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnsemblOverlapSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EnsemblOverlapSpec = serde_json::from_value(spec)?;
        if spec.features.is_empty() {
            return Err(dag_core::registry::error::Error::Unknown(
                "source_ensembl_overlap requires at least one feature type".into(),
            ));
        }
        Ok(Box::new(EnsemblOverlapNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EnsemblOverlapNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_ensembl_overlap"
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
        let operation = "Ensembl overlap";
        let features = client(self.spec.endpoint.as_deref())
            .overlap_region(&self.spec.species, &self.spec.region, &self.spec.features)
            .await
            .map_err(|error| err(operation, error))?;
        emit(
            ctx,
            operation,
            ensembl::convert::overlap_table(&features)
                .map_err(|error| invalid(operation, format!("normalize response: {error}")))?,
        )
        .await
    }
}

/// Input mode for `source_ensembl_sequence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnsemblSequenceMode {
    /// Fetch by a stable Ensembl ID.
    Id,
    /// Fetch by species and genomic region.
    Region,
}

/// Sequence type accepted by Ensembl sequence endpoints.
///
/// The SDK type already provides schema metadata and the `snake_case` wire
/// representation expected by this node.
type EnsemblSequenceType = SequenceType;

fn default_sequence_type() -> EnsemblSequenceType {
    EnsemblSequenceType::Dna
}

/// Spec for `source_ensembl_sequence`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EnsemblSequenceSpec {
    /// Whether `id` or `region` selects the sequence.
    #[serde(default = "default_sequence_mode")]
    pub mode: EnsemblSequenceMode,
    /// Stable Ensembl ID used when `mode=id`.
    #[serde(default)]
    pub id: Option<String>,
    /// Species used when `mode=region`.
    #[serde(default)]
    pub species: Option<String>,
    /// Genomic region used when `mode=region`.
    #[serde(default)]
    pub region: Option<String>,
    /// Sequence type (`dna`, `cdna`, `cds`, or `protein`).
    #[serde(default = "default_sequence_type")]
    pub sequence_type: EnsemblSequenceType,
    /// Bases to expand upstream, where valid for the sequence type.
    #[serde(default)]
    pub expand_5prime: Option<u64>,
    /// Bases to expand downstream, where valid for the sequence type.
    #[serde(default)]
    pub expand_3prime: Option<u64>,
    /// Endpoint override for tests and private Ensembl mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

fn default_sequence_mode() -> EnsemblSequenceMode {
    EnsemblSequenceMode::Id
}

/// Ensembl sequence source.
#[derive(Clone)]
pub struct EnsemblSequenceNode {
    meta: NodePorts,
    spec: EnsemblSequenceSpec,
}

pub struct EnsemblSequenceNodeFactory;

impl NodeFactory for EnsemblSequenceNodeFactory {
    fn kind(&self) -> &'static str {
        "source_ensembl_sequence"
    }

    fn desc(&self) -> &'static str {
        "Fetch genomic, cDNA, CDS, or protein sequence."
    }

    fn doc(&self) -> &'static str {
        "Calls `/sequence/id/{id}` or `/sequence/region/{species}/{region}` \
         and emits one row containing the query, stable ID, description, \
         molecule, version, sequence, and sequence statistics."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnsemblSequenceSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EnsemblSequenceSpec = serde_json::from_value(spec)?;
        match spec.mode {
            EnsemblSequenceMode::Id if spec.id.as_deref().unwrap_or("").trim().is_empty() => {
                return Err(dag_core::registry::error::Error::Unknown(
                    "source_ensembl_sequence requires id when mode=id".into(),
                ));
            }
            EnsemblSequenceMode::Region
                if spec.species.as_deref().unwrap_or("").trim().is_empty()
                    || spec.region.as_deref().unwrap_or("").trim().is_empty() =>
            {
                return Err(dag_core::registry::error::Error::Unknown(
                    "source_ensembl_sequence requires species and region when mode=region".into(),
                ));
            }
            _ => {}
        }
        Ok(Box::new(EnsemblSequenceNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EnsemblSequenceNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_ensembl_sequence"
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
        let operation = "Ensembl sequence";
        let client = client(self.spec.endpoint.as_deref());
        let sequence = match self.spec.mode {
            EnsemblSequenceMode::Id => {
                let id = self.spec.id.as_deref().unwrap_or_default();
                client
                    .sequence_id(
                        id,
                        self.spec.sequence_type,
                        self.spec.expand_5prime,
                        self.spec.expand_3prime,
                    )
                    .await
            }
            EnsemblSequenceMode::Region => {
                let species = self.spec.species.as_deref().unwrap_or_default();
                let region = self.spec.region.as_deref().unwrap_or_default();
                client
                    .sequence_region(
                        species,
                        region,
                        self.spec.sequence_type,
                        self.spec.expand_5prime,
                        self.spec.expand_3prime,
                    )
                    .await
            }
        }
        .map_err(|error| err(operation, error))?;

        let stats = ensembl::nucleotide_stats(&sequence.seq);
        let fields = vec![
            Field::new("query", DataType::Utf8, false),
            Field::new("id", DataType::Utf8, true),
            Field::new("description", DataType::Utf8, true),
            Field::new("molecule", DataType::Utf8, true),
            Field::new("version", DataType::UInt32, true),
            Field::new("sequence", DataType::Utf8, false),
            Field::new("length", DataType::UInt32, false),
            Field::new("a", DataType::UInt32, false),
            Field::new("c", DataType::UInt32, false),
            Field::new("g", DataType::UInt32, false),
            Field::new("t", DataType::UInt32, false),
            Field::new("n", DataType::UInt32, false),
            Field::new("gc_fraction", DataType::Float64, false),
        ];
        let arrays = vec![
            strings(vec![Some(sequence.query.clone())]),
            strings(vec![sequence.id.clone()]),
            strings(vec![sequence.desc.clone()]),
            strings(vec![sequence.molecule.clone()]),
            opt_u32(vec![sequence.version]),
            strings(vec![Some(sequence.seq.clone())]),
            Arc::new(UInt32Array::from(vec![Some(stats.length as u32)])),
            Arc::new(UInt32Array::from(vec![Some(stats.a as u32)])),
            Arc::new(UInt32Array::from(vec![Some(stats.c as u32)])),
            Arc::new(UInt32Array::from(vec![Some(stats.g as u32)])),
            Arc::new(UInt32Array::from(vec![Some(stats.t as u32)])),
            Arc::new(UInt32Array::from(vec![Some(stats.n as u32)])),
            opt_f64(vec![Some(stats.gc_fraction)]),
        ];
        let value = batch(fields, arrays)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe(ctx, operation, value)?);
        Ok(outputs)
    }
}

/// Spec for `source_ensembl_vep`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EnsemblVepSpec {
    /// Species name, alias, common name, or taxonomy ID.
    pub species: String,
    /// Variant/rs ID. Omit when using region plus allele.
    #[serde(default)]
    pub id: Option<String>,
    /// Genomic region when `id` is omitted.
    #[serde(default)]
    pub region: Option<String>,
    /// Alternate allele when `id` is omitted.
    #[serde(default)]
    pub allele: Option<String>,
    /// Endpoint override for tests and private Ensembl mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Ensembl VEP annotation source.
#[derive(Clone)]
pub struct EnsemblVepNode {
    meta: NodePorts,
    spec: EnsemblVepSpec,
}

pub struct EnsemblVepNodeFactory;

impl NodeFactory for EnsemblVepNodeFactory {
    fn kind(&self) -> &'static str {
        "source_ensembl_vep"
    }

    fn desc(&self) -> &'static str {
        "Annotate one variant with Ensembl VEP."
    }

    fn doc(&self) -> &'static str {
        "Calls `/vep/{species}/id/{id}` or the region+allele form and emits \
         one row per transcript consequence. Results without a transcript \
         consequence still emit a top-level row."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnsemblVepSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EnsemblVepSpec = serde_json::from_value(spec)?;
        let has_id = spec
            .id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        let has_region = spec
            .region
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        let has_allele = spec
            .allele
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        if !(has_id || has_region && has_allele) {
            return Err(dag_core::registry::error::Error::Unknown(
                "source_ensembl_vep requires id, or region plus allele".into(),
            ));
        }
        Ok(Box::new(EnsemblVepNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EnsemblVepNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_ensembl_vep"
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
        let operation = "Ensembl VEP";
        let client = client(self.spec.endpoint.as_deref());
        let results = if let Some(id) = self.spec.id.as_deref() {
            client.vep_id(&self.spec.species, id).await
        } else {
            let region = self.spec.region.as_deref().unwrap_or_default();
            let allele = self.spec.allele.as_deref().unwrap_or_default();
            client.vep_region(&self.spec.species, region, allele).await
        }
        .map_err(|error| err(operation, error))?;
        emit(
            ctx,
            operation,
            ensembl::convert::vep_table(&results)
                .map_err(|error| invalid(operation, format!("normalize response: {error}")))?,
        )
        .await
    }
}

/// Spec for `source_ensembl_variation`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EnsemblVariationSpec {
    /// Species name, alias, common name, or taxonomy ID.
    pub species: String,
    /// Variant/rs identifier.
    pub id: String,
    /// Endpoint override for tests and private Ensembl mirrors.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Ensembl variation source.
#[derive(Clone)]
pub struct EnsemblVariationNode {
    meta: NodePorts,
    spec: EnsemblVariationSpec,
}

pub struct EnsemblVariationNodeFactory;

impl NodeFactory for EnsemblVariationNodeFactory {
    fn kind(&self) -> &'static str {
        "source_ensembl_variation"
    }

    fn desc(&self) -> &'static str {
        "Fetch an Ensembl variation and its coordinate mappings."
    }

    fn doc(&self) -> &'static str {
        "Calls `/variation/{species}/{id}` and emits one row per assembly \
         mapping with clinical significance, class, consequence, allele \
         frequency, location, and ancestral allele."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EnsemblVariationSpec)
    }

    fn ports(&self) -> NodePorts {
        output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value(spec)?;
        Ok(Box::new(EnsemblVariationNode {
            meta: output_port(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for EnsemblVariationNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_ensembl_variation"
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
        let operation = "Ensembl variation";
        let variation = client(self.spec.endpoint.as_deref())
            .variation(&self.spec.species, &self.spec.id)
            .await
            .map_err(|error| err(operation, error))?;
        emit(
            ctx,
            operation,
            ensembl::convert::variation_table(&variation)
                .map_err(|error| invalid(operation, format!("normalize response: {error}")))?,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use arrow_array::Array;
    use dag_core::dag::node_event::NodeReporter;
    use datafusion::prelude::SessionContext;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_json_stub(body: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                if read == 0 || request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        endpoint
    }

    async fn first_batch(outputs: &PortOutputs) -> RecordBatch {
        outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap()
            .remove(0)
    }

    fn utf8(batch: &RecordBatch, name: &str, row: usize) -> String {
        let column = batch.schema().index_of(name).unwrap();
        batch
            .column(column)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(row)
            .to_string()
    }

    #[tokio::test]
    async fn species_node_filters_and_emits_typed_rows() {
        let endpoint = spawn_json_stub(
            serde_json::json!({
                "species": [
                    {"name":"homo_sapiens","display_name":"Human","common_name":"human","division":"EnsemblVertebrates","release":114,"assembly":"GRCh38","accession":"GCA_000001405.29","taxon_id":"9606","aliases":["human"],"groups":["core"]},
                    {"name":"mus_musculus","display_name":"Mouse","common_name":"mouse","division":"EnsemblVertebrates","release":114,"assembly":"GRCm39","taxon_id":"10090","groups":["core"]}
                ]
            })
            .to_string(),
        )
        .await;
        let session = SessionContext::new();
        let ctx = NodeCtx::new(session.runtime_env(), None);
        let mut node = EnsemblSpeciesNodeFactory
            .build(
                serde_json::json!({"query":"human", "endpoint":endpoint}),
                ctx.clone(),
            )
            .unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let batch = first_batch(&outputs).await;
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(utf8(&batch, "name", 0), "homo_sapiens");
        assert_eq!(utf8(&batch, "assembly", 0), "GRCh38");
    }

    #[tokio::test]
    async fn lookup_node_expands_stable_id() {
        let endpoint = spawn_json_stub(
            serde_json::json!({
                "id":"ENSG00000139618", "object_type":"Gene", "display_name":"BRCA2",
                "species":"homo_sapiens", "start":32315086, "end":32400268,
                "strand":1, "seq_region_name":"13", "assembly_name":"GRCh38"
            })
            .to_string(),
        )
        .await;
        let session = SessionContext::new();
        let ctx = NodeCtx::new(session.runtime_env(), None);
        let mut node = EnsemblLookupNodeFactory
            .build(
                serde_json::json!({"ids":["ENSG00000139618"], "expand":true, "endpoint":endpoint}),
                ctx.clone(),
            )
            .unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let batch = first_batch(&outputs).await;
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(utf8(&batch, "id", 0), "ENSG00000139618");
        assert_eq!(utf8(&batch, "display_name", 0), "BRCA2");
    }

    #[tokio::test]
    async fn sequence_node_emits_sequence_and_stats() {
        let endpoint = spawn_json_stub(
            serde_json::json!({
                "query":"ENST00000380152", "id":"ENST00000380152", "seq":"ACGTN",
                "desc":"BRCA2", "molecule":"cdna", "version":3
            })
            .to_string(),
        )
        .await;
        let session = SessionContext::new();
        let ctx = NodeCtx::new(session.runtime_env(), None);
        let mut node = EnsemblSequenceNodeFactory
            .build(
                serde_json::json!({
                    "mode":"id", "id":"ENST00000380152", "sequence_type":"cdna",
                    "endpoint":endpoint
                }),
                ctx.clone(),
            )
            .unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let batch = first_batch(&outputs).await;
        assert_eq!(utf8(&batch, "sequence", 0), "ACGTN");
        let column = batch.schema().index_of("gc_fraction").unwrap();
        let values = batch
            .column(column)
            .as_any()
            .downcast_ref::<arrow_array::Float64Array>()
            .unwrap();
        assert!((values.value(0) - 0.4).abs() < 1e-12);
    }

    #[tokio::test]
    async fn batch_lookup_preserves_request_order() {
        let endpoint = spawn_json_stub(
            serde_json::json!({
                "ENST00000380152": {"id":"ENST00000380152", "start":1, "end":2, "strand":1, "seq_region_name":"13"},
                "ENSG00000139618": {"id":"ENSG00000139618", "start":3, "end":4, "strand":1, "seq_region_name":"13"}
            })
            .to_string(),
        )
        .await;
        let session = SessionContext::new();
        let ctx = NodeCtx::new(session.runtime_env(), None);
        let mut node = EnsemblLookupNodeFactory
            .build(
                serde_json::json!({
                    "ids":["ENSG00000139618","ENST00000380152"],
                    "endpoint":endpoint
                }),
                ctx.clone(),
            )
            .unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let batch = first_batch(&outputs).await;
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(utf8(&batch, "id", 0), "ENSG00000139618");
        assert_eq!(utf8(&batch, "id", 1), "ENST00000380152");
    }

    #[test]
    fn empty_dynamic_tables_keep_a_valid_schema() {
        let table = ensembl::convert::Table::from_json_rows(&[]).unwrap();
        let batch = table_to_batch("empty Ensembl response", &table).unwrap();
        assert_eq!(batch.num_columns(), 1);
        assert_eq!(batch.num_rows(), 0);
        assert_eq!(batch.schema().field(0).name(), "response");
    }

    #[tokio::test]
    async fn vep_node_expands_transcript_consequences() {
        let endpoint = spawn_json_stub(
            serde_json::json!([{
                "id":"rs80357906", "input":"rs80357906",
                "most_severe_consequence":"frameshift_variant",
                "transcript_consequences":[
                    {"transcript_id":"ENST00000352993", "gene_symbol":"BRCA1"},
                    {"transcript_id":"ENST00000357670", "gene_symbol":"BRCA1"}
                ]
            }])
            .to_string(),
        )
        .await;
        let session = SessionContext::new();
        let ctx = NodeCtx::new(session.runtime_env(), None);
        let mut node = EnsemblVepNodeFactory
            .build(
                serde_json::json!({"species":"human", "id":"rs80357906", "endpoint":endpoint}),
                ctx.clone(),
            )
            .unwrap();
        let outputs = node
            .execute(&ctx, &[], &NodeReporter::noop())
            .await
            .unwrap();
        let batch = first_batch(&outputs).await;
        assert_eq!(batch.num_rows(), 2);
        assert!(utf8(&batch, "transcript_consequence", 0).contains("ENST00000352993"));
    }
}
