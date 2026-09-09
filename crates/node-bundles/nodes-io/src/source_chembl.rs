//! ChEMBL source nodes for structured bioactivity and compound tables.
//!
//! - [`ChemblActivitiesNode`] (`source_chembl_activities`) - standardized
//!   activity measurements for a molecule, target, assay, or document.
//! - [`ChemblMoleculesNode`] (`source_chembl_molecules`) - molecule search
//!   hits with structural and physicochemical properties.

use std::sync::Arc;

use arrow_array::{Array, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use chembl::{Activity, ChEMBLClient, Molecule, ResourceQuery};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

const ACTIVITY_ONLY: [&str; 20] = [
    "activity_id",
    "assay_chembl_id",
    "assay_description",
    "assay_type",
    "document_chembl_id",
    "molecule_chembl_id",
    "molecule_pref_name",
    "parent_molecule_chembl_id",
    "pchembl_value",
    "relation",
    "standard_flag",
    "standard_relation",
    "standard_type",
    "standard_units",
    "standard_value",
    "standard_upper_value",
    "target_chembl_id",
    "target_organism",
    "target_pref_name",
    "target_tax_id",
];

const MOLECULE_ONLY: [&str; 12] = [
    "molecule_chembl_id",
    "pref_name",
    "molecule_type",
    "max_phase",
    "first_approval",
    "withdrawn_flag",
    "molecule_properties",
    "molecule_structures",
    "atc_classifications",
    "oral",
    "parenteral",
    "topical",
];

fn str_array(rows: Vec<Option<String>>) -> Arc<dyn Array> {
    let refs: Vec<Option<&str>> = rows.iter().map(|value| value.as_deref()).collect();
    Arc::new(StringArray::from(refs))
}

fn f64_array(rows: Vec<Option<f64>>) -> Arc<dyn Array> {
    Arc::new(Float64Array::from(rows))
}

fn i64_array(rows: Vec<Option<i64>>) -> Arc<dyn Array> {
    Arc::new(Int64Array::from(rows))
}

fn bool_array(rows: Vec<Option<bool>>) -> Arc<dyn Array> {
    Arc::new(BooleanArray::from(rows))
}

fn text_field(name: &str) -> Field {
    Field::new(name, DataType::Utf8, true)
}

fn float_field(name: &str) -> Field {
    Field::new(name, DataType::Float64, true)
}

fn batch_to_df(ctx: &SessionContext, batch: RecordBatch) -> Result<DataFrame, DagError> {
    ctx.read_batch(batch)
        .map_err(|error| DagError::Schedule(format!("failed to read ChEMBL batch: {error}")))
}

fn client_from_endpoint(endpoint: &Option<String>) -> ChEMBLClient {
    match endpoint {
        Some(url) => ChEMBLClient::with_endpoint(url),
        None => ChEMBLClient::new(),
    }
}

fn request_error(error: chembl::ChemblError) -> DagError {
    DagError::Schedule(format!("ChEMBL request failed: {error}"))
}

fn optional_filter(query: ResourceQuery, field: &str, value: &Option<String>) -> ResourceQuery {
    match value.as_deref() {
        Some(value) if !value.is_empty() => query.filter(field, value),
        _ => query,
    }
}

// ===========================================================================
// Activities
// ===========================================================================

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ChemblActivitiesSpec {
    /// ChEMBL molecule ID, e.g. `CHEMBL25`.
    #[serde(default)]
    pub molecule_chembl_id: Option<String>,
    /// ChEMBL target ID, e.g. `CHEMBL2094253`.
    #[serde(default)]
    pub target_chembl_id: Option<String>,
    /// ChEMBL assay ID, e.g. `CHEMBL663853`.
    #[serde(default)]
    pub assay_chembl_id: Option<String>,
    /// ChEMBL document ID, e.g. `CHEMBL1137930`.
    #[serde(default)]
    pub document_chembl_id: Option<String>,
    /// Exact standardized endpoint type, e.g. `IC50`.
    #[serde(default)]
    pub standard_type: Option<String>,
    /// Drop rows whose pChEMBL is lower than this value.
    #[serde(default)]
    pub min_pchembl: Option<f64>,
    /// Fetch every matching page, up to `max_records`.
    #[serde(default)]
    pub fetch_all: Option<bool>,
    /// Maximum rows retained when `fetch_all=true` (default 10000).
    #[serde(default)]
    pub max_records: Option<u32>,
    /// Page size when `fetch_all=false` (default 1000, max 1000).
    #[serde(default)]
    pub size: Option<u32>,
    /// Zero-based offset when `fetch_all=false`.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Override the ChEMBL REST endpoint (tests / mirrors).
    #[serde(default)]
    pub endpoint: Option<String>,
}

#[derive(Clone)]
pub struct ChemblActivitiesNode {
    meta: NodePorts,
    spec: ChemblActivitiesSpec,
}

pub struct ChemblActivitiesNodeFactory;

fn source_ports() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for ChemblActivitiesNodeFactory {
    fn kind(&self) -> &'static str {
        "source_chembl_activities"
    }

    fn desc(&self) -> &'static str {
        "Fetches standardized ChEMBL bioactivities as a typed table."
    }

    fn doc(&self) -> &'static str {
        "A source node that fetches standardized ChEMBL activity measurements and emits \
        a DataFrame. Provide at least one filter: molecule_chembl_id, target_chembl_id, \
        assay_chembl_id, or document_chembl_id.\n\n\
        Output schema: activity_id, molecule_id, target_id, assay_id, document_id, \
        assay_description, standard_type, standard_relation, standard_value, \
        standard_units, pchembl_value, target_name, target_organism.\n\n\
        By default this retrieves up to 10000 rows across pages. Pipe the output into \
        `sql_node`, statistics nodes, or `dataframe_to_file`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ChemblActivitiesSpec)
    }

    fn ports(&self) -> NodePorts {
        source_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: ChemblActivitiesSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ChemblActivitiesNode::node(spec)))
    }
}

impl ChemblActivitiesNode {
    pub fn node(spec: ChemblActivitiesSpec) -> Self {
        Self {
            meta: source_ports(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for ChemblActivitiesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_chembl_activities"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let spec = &self.spec;
        let has_filter = [
            &spec.molecule_chembl_id,
            &spec.target_chembl_id,
            &spec.assay_chembl_id,
            &spec.document_chembl_id,
        ]
        .iter()
        .any(|value| value.as_deref().is_some_and(|value| !value.is_empty()));
        if !has_filter {
            return Err(DagError::Schedule(
                "source_chembl_activities requires molecule_chembl_id, target_chembl_id, \
                 assay_chembl_id, or document_chembl_id"
                    .to_string(),
            ));
        }

        let client = client_from_endpoint(&spec.endpoint);
        let base = ResourceQuery::new().only(ACTIVITY_ONLY);
        let base = optional_filter(base, "molecule_chembl_id", &spec.molecule_chembl_id);
        let base = optional_filter(base, "target_chembl_id", &spec.target_chembl_id);
        let base = optional_filter(base, "assay_chembl_id", &spec.assay_chembl_id);
        let base = optional_filter(base, "document_chembl_id", &spec.document_chembl_id);
        let base = optional_filter(base, "standard_type", &spec.standard_type);

        let mut rows = if spec.fetch_all.unwrap_or(true) {
            let mut result = Vec::new();
            let mut offset = 0_u32;
            let limit = 1000_u32;
            loop {
                let query = base.clone().limit(limit).offset(offset);
                let page: chembl::Page<Activity> = client
                    .list("activity", &query)
                    .await
                    .map_err(request_error)?;
                let returned = page.records.len();
                result.extend(page.records);
                if returned == 0
                    || page.page_meta.next.is_none()
                    || result.len() >= spec.max_records.unwrap_or(10_000).max(1) as usize
                {
                    break;
                }
                offset += returned as u32;
            }
            result
        } else {
            let query = base
                .limit(spec.size.unwrap_or(1000).min(1000))
                .offset(spec.offset.unwrap_or(0));
            let page: chembl::Page<Activity> = client
                .list("activity", &query)
                .await
                .map_err(request_error)?;
            page.records
        };

        if let Some(minimum) = spec.min_pchembl {
            rows.retain(|activity| activity.pchembl_value.is_some_and(|value| value >= minimum));
        }
        let maximum = spec.max_records.unwrap_or(10_000).max(1) as usize;
        rows.truncate(maximum);

        let batch = build_activity_batch(rows)?;
        let dataframe = batch_to_df(&ctx.session(), batch)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

#[allow(clippy::too_many_lines)]
fn build_activity_batch(rows: Vec<Activity>) -> Result<RecordBatch, DagError> {
    let ids: Vec<Option<i64>> = rows
        .iter()
        .map(|row| Some(row.activity_id as i64))
        .collect();
    let molecule_ids: Vec<Option<String>> = rows
        .iter()
        .map(|row| Some(row.molecule_chembl_id.clone()))
        .collect();
    let target_ids: Vec<Option<String>> = rows
        .iter()
        .map(|row| Some(row.target_chembl_id.clone()))
        .collect();
    let assay_ids: Vec<Option<String>> = rows
        .iter()
        .map(|row| Some(row.assay_chembl_id.clone()))
        .collect();
    let document_ids: Vec<Option<String>> = rows
        .iter()
        .map(|row| row.document_chembl_id.clone())
        .collect();
    let assay_descriptions: Vec<Option<String>> = rows
        .iter()
        .map(|row| row.assay_description.clone())
        .collect();
    let standard_types: Vec<Option<String>> =
        rows.iter().map(|row| row.standard_type.clone()).collect();
    let standard_relations: Vec<Option<String>> = rows
        .iter()
        .map(|row| row.standard_relation.clone())
        .collect();
    let standard_values: Vec<Option<f64>> = rows.iter().map(|row| row.standard_value).collect();
    let standard_units: Vec<Option<String>> =
        rows.iter().map(|row| row.standard_units.clone()).collect();
    let pchembl_values: Vec<Option<f64>> = rows.iter().map(|row| row.pchembl_value).collect();
    let target_names: Vec<Option<String>> = rows
        .iter()
        .map(|row| row.target_pref_name.clone())
        .collect();
    let target_organisms: Vec<Option<String>> =
        rows.iter().map(|row| row.target_organism.clone()).collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("activity_id", DataType::Int64, true),
        text_field("molecule_id"),
        text_field("target_id"),
        text_field("assay_id"),
        text_field("document_id"),
        text_field("assay_description"),
        text_field("standard_type"),
        text_field("standard_relation"),
        float_field("standard_value"),
        text_field("standard_units"),
        float_field("pchembl_value"),
        text_field("target_name"),
        text_field("target_organism"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            i64_array(ids),
            str_array(molecule_ids),
            str_array(target_ids),
            str_array(assay_ids),
            str_array(document_ids),
            str_array(assay_descriptions),
            str_array(standard_types),
            str_array(standard_relations),
            f64_array(standard_values),
            str_array(standard_units),
            f64_array(pchembl_values),
            str_array(target_names),
            str_array(target_organisms),
        ],
    )
    .map_err(|error| DagError::Schedule(format!("failed to build ChEMBL activity batch: {error}")))
}

// ===========================================================================
// Molecule search
// ===========================================================================

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ChemblMoleculesSpec {
    /// Search term, typically a preferred name, synonym, or research code.
    pub query: String,
    /// Fetch every search page, up to `max_records`.
    #[serde(default)]
    pub fetch_all: Option<bool>,
    /// Maximum rows retained when `fetch_all=true` (default 10000).
    #[serde(default)]
    pub max_records: Option<u32>,
    /// Page size when `fetch_all=false` (default 100, max 1000).
    #[serde(default)]
    pub size: Option<u32>,
    /// Zero-based offset when `fetch_all=false`.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Override the ChEMBL REST endpoint (tests / mirrors).
    #[serde(default)]
    pub endpoint: Option<String>,
}

#[derive(Clone)]
pub struct ChemblMoleculesNode {
    meta: NodePorts,
    spec: ChemblMoleculesSpec,
}

pub struct ChemblMoleculesNodeFactory;

impl NodeFactory for ChemblMoleculesNodeFactory {
    fn kind(&self) -> &'static str {
        "source_chembl_molecules"
    }

    fn desc(&self) -> &'static str {
        "Searches ChEMBL molecules and emits compound metadata as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node that searches ChEMBL molecules and emits a stable compound table. \
        Use it to resolve names or research codes to molecule IDs before activity joins.\n\n\
        Output schema: molecule_id, pref_name, molecule_type, max_phase, first_approval, \
        molecular_weight, formula, canonical_smiles, inchi_key, withdrawn.\n\n\
        The default page size is 100. Set fetch_all=true to walk search pages, capped by \
        max_records."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ChemblMoleculesSpec)
    }

    fn ports(&self) -> NodePorts {
        source_ports()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: ChemblMoleculesSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ChemblMoleculesNode::node(spec)))
    }
}

impl ChemblMoleculesNode {
    pub fn node(spec: ChemblMoleculesSpec) -> Self {
        Self {
            meta: source_ports(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for ChemblMoleculesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_chembl_molecules"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = client_from_endpoint(&self.spec.endpoint);
        let query = ResourceQuery::new().only(MOLECULE_ONLY);
        let maximum = self.spec.max_records.unwrap_or(10_000).max(1) as usize;
        let rows = if self.spec.fetch_all.unwrap_or(false) {
            let mut result = Vec::new();
            let mut offset = 0_u32;
            loop {
                let page = client
                    .molecules_by_name(&self.spec.query, &query.clone().limit(1000).offset(offset))
                    .await
                    .map_err(request_error)?;
                let returned = page.records.len();
                result.extend(page.records);
                if returned == 0 || page.page_meta.next.is_none() || result.len() >= maximum {
                    break;
                }
                offset += returned as u32;
            }
            result
        } else {
            client
                .molecules_by_name(
                    &self.spec.query,
                    &query
                        .limit(self.spec.size.unwrap_or(100).min(1000))
                        .offset(self.spec.offset.unwrap_or(0)),
                )
                .await
                .map_err(request_error)?
                .records
        };
        let mut rows = rows;
        rows.truncate(maximum);

        let batch = build_molecule_batch(rows)?;
        let dataframe = batch_to_df(&ctx.session(), batch)?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, dataframe);
        Ok(outputs)
    }
}

fn build_molecule_batch(rows: Vec<Molecule>) -> Result<RecordBatch, DagError> {
    let ids: Vec<Option<String>> = rows
        .iter()
        .map(|row| Some(row.molecule_chembl_id.clone()))
        .collect();
    let names: Vec<Option<String>> = rows.iter().map(|row| row.pref_name.clone()).collect();
    let types: Vec<Option<String>> = rows.iter().map(|row| row.molecule_type.clone()).collect();
    let max_phases: Vec<Option<f64>> = rows.iter().map(|row| row.max_phase).collect();
    let first_approvals: Vec<Option<f64>> = rows.iter().map(|row| row.first_approval).collect();
    let molecular_weights: Vec<Option<f64>> = rows
        .iter()
        .map(|row| row.molecule_properties.as_ref().and_then(|p| p.full_mwt))
        .collect();
    let formulas: Vec<Option<String>> = rows
        .iter()
        .map(|row| {
            row.molecule_properties
                .as_ref()
                .and_then(|p| p.full_molformula.clone())
        })
        .collect();
    let smiles: Vec<Option<String>> = rows
        .iter()
        .map(|row| {
            row.molecule_structures
                .as_ref()
                .and_then(|s| s.canonical_smiles.clone())
        })
        .collect();
    let inchi_keys: Vec<Option<String>> = rows
        .iter()
        .map(|row| {
            row.molecule_structures
                .as_ref()
                .and_then(|s| s.standard_inchi_key.clone())
        })
        .collect();
    let withdrawn: Vec<Option<bool>> = rows.iter().map(|row| Some(row.withdrawn_flag)).collect();

    let schema = Arc::new(Schema::new(vec![
        text_field("molecule_id"),
        text_field("pref_name"),
        text_field("molecule_type"),
        float_field("max_phase"),
        float_field("first_approval"),
        float_field("molecular_weight"),
        text_field("formula"),
        text_field("canonical_smiles"),
        text_field("inchi_key"),
        Field::new("withdrawn", DataType::Boolean, true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(ids),
            str_array(names),
            str_array(types),
            f64_array(max_phases),
            f64_array(first_approvals),
            f64_array(molecular_weights),
            str_array(formulas),
            str_array(smiles),
            str_array(inchi_keys),
            bool_array(withdrawn),
        ],
    )
    .map_err(|error| DagError::Schedule(format!("failed to build ChEMBL molecule batch: {error}")))
}
