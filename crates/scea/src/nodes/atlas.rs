//! SCEA Atlas source nodes.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use crate::SceaClient;
use crate::nodes::util::{
    dataframe_output, dataframe_port, f64, f64_array, request_error, schedule_error, str_array,
    u32, u32_array, u64, u64_array, utf8,
};
use crate::types::{Experiment, ExperimentFilter, ExperimentPageContent, MarkerGene};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_scea_factory {
    ($factory:ident, $node:ident, $spec:ident, $kind:literal, $description:literal, $documentation:literal) => {
        impl NodeFactory for $factory {
            fn kind(&self) -> &'static str {
                $kind
            }

            fn desc(&self) -> &'static str {
                $description
            }

            fn doc(&self) -> &'static str {
                $documentation
            }

            fn spec_schema(&self) -> schemars::Schema {
                schema_for!($spec)
            }

            fn ports(&self) -> NodePorts {
                dataframe_port()
            }

            fn build(
                &self,
                spec: serde_json::Value,
                _node_ctx: NodeCtx,
            ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
                let spec = serde_json::from_value::<$spec>(spec)?;
                Ok(Box::new($node {
                    meta: dataframe_port(),
                    spec,
                }))
            }
        }
    };
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct SceaExperimentsSpec {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub species: Option<String>,
    #[serde(default)]
    pub kingdom: Option<String>,
    #[serde(default)]
    pub experiment_type: Option<String>,
    #[serde(default)]
    pub max_results: Option<u32>,
}

#[derive(Clone)]
pub struct SceaExperimentsNode {
    meta: NodePorts,
    spec: SceaExperimentsSpec,
}

pub struct SceaExperimentsNodeFactory;

impl_scea_factory!(
    SceaExperimentsNodeFactory,
    SceaExperimentsNode,
    SceaExperimentsSpec,
    "source_scea_experiments",
    "Lists EBI Single Cell Expression Atlas experiments.",
    "A source node over the SCEA json/experiments index. Supports local text, \
     species, kingdom, experiment-type, and row-limit filters. The public API \
     ignores server-side filter parameters, so filters are applied after the \
     official index is fetched."
);

#[async_trait]
impl DagNode for SceaExperimentsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_scea_experiments"
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
        let filter = ExperimentFilter {
            query: self.spec.query.clone(),
            species: self.spec.species.clone(),
            kingdom: self.spec.kingdom.clone(),
            experiment_type: self.spec.experiment_type.clone(),
            max_results: self.spec.max_results.map(|value| value as usize),
        };
        let experiments = SceaClient::new()
            .experiments(&filter)
            .await
            .map_err(|error| request_error(error, "source_scea_experiments"))?;
        let batch = build_experiment_batch(&experiments)?;
        dataframe_output(ctx, batch, "source_scea_experiments")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct SceaExperimentSpec {
    pub accession: String,
}

#[derive(Clone)]
pub struct SceaExperimentNode {
    meta: NodePorts,
    spec: SceaExperimentSpec,
}

pub struct SceaExperimentNodeFactory;

impl_scea_factory!(
    SceaExperimentNodeFactory,
    SceaExperimentNode,
    SceaExperimentSpec,
    "source_scea_experiment",
    "Fetches one Single Cell Expression Atlas experiment page contract.",
    "Extracts the official experiment-page content JSON for an accession. \
     Output includes available clustering k values, marker metadata fields, \
     units, supplementary-information tabs, and downloadable file descriptors. \
     No expression matrix or archive is downloaded."
);

#[async_trait]
impl DagNode for SceaExperimentNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_scea_experiment"
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
        let content = SceaClient::new()
            .experiment_page(&self.spec.accession)
            .await
            .map_err(|error| request_error(error, "source_scea_experiment"))?;
        let batch = build_page_batch(&content)?;
        dataframe_output(ctx, batch, "source_scea_experiment")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct SceaMarkersSpec {
    pub accession: String,
    /// Clustering resolution k used by SCEA marker-gene results.
    pub k: u32,
}

#[derive(Clone)]
pub struct SceaMarkersNode {
    meta: NodePorts,
    spec: SceaMarkersSpec,
}

pub struct SceaMarkersNodeFactory;

impl_scea_factory!(
    SceaMarkersNodeFactory,
    SceaMarkersNode,
    SceaMarkersSpec,
    "source_scea_markers",
    "Fetches Single Cell Expression Atlas cluster marker genes.",
    "A source node over json/experiments/{accession}/marker-genes/clusters. \
     Each row is one gene-by-cluster matrix cell and includes gene, cluster, \
     marker cluster, expression value, p-value, and expression unit."
);

#[async_trait]
impl DagNode for SceaMarkersNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_scea_markers"
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
        let markers = SceaClient::new()
            .marker_genes(&self.spec.accession, self.spec.k)
            .await
            .map_err(|error| request_error(error, "source_scea_markers"))?;
        let batch = build_marker_batch(&markers, &self.spec.accession, self.spec.k)?;
        dataframe_output(ctx, batch, "source_scea_markers")
    }
}

fn joined(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join("; "))
}

fn build_experiment_batch(rows: &[Experiment]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("accession"),
        utf8("description"),
        utf8("species"),
        utf8("kingdom"),
        utf8("load_date"),
        utf8("last_update"),
        utf8("raw_experiment_type"),
        utf8("technology_type"),
        u64("assay_count"),
        utf8("experimental_factors"),
        utf8("projects"),
        utf8("experiment_type"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.accession.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.description.clone()))
                    .collect(),
            ),
            str_array(rows.iter().map(|row| Some(row.species.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.kingdom.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.load_date.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.last_update.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.raw_experiment_type.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| joined(&row.technology_type))
                    .collect(),
            ),
            u64_array(rows.iter().map(|row| row.number_of_assays).collect()),
            str_array(
                rows.iter()
                    .map(|row| joined(&row.experimental_factors))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| joined(&row.experiment_projects))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.experiment_type.clone()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_scea_experiments", error))
}

fn build_page_batch(content: &ExperimentPageContent) -> Result<RecordBatch, DagError> {
    let results = tab_props(content, "results");
    let supplementary = tab_props(content, "supplementary-information");
    let downloads = tab_props(content, "downloads");
    let ks = results.as_ref().and_then(|props| props.get("ks").cloned());
    let marker_ks = results
        .as_ref()
        .and_then(|props| props.get("ksWithMarkerGenes").cloned());
    let metadata = results
        .as_ref()
        .and_then(|props| props.get("metadata").cloned());
    let marker_metadata = results
        .as_ref()
        .and_then(|props| props.get("markerGeneMetadata").cloned());
    let units = results
        .as_ref()
        .and_then(|props| props.get("units").cloned());

    let schema = Arc::new(Schema::new(vec![
        utf8("accession"),
        utf8("species"),
        utf8("access_key"),
        utf8("disclaimer"),
        utf8("ks_json"),
        utf8("marker_k_json"),
        utf8("metadata_json"),
        utf8("marker_metadata_json"),
        utf8("units_json"),
        utf8("downloads_json"),
        utf8("supplementary_json"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(content.accession.clone())]),
            str_array(vec![Some(content.species.clone())]),
            str_array(vec![Some(content.access_key.clone())]),
            str_array(vec![Some(content.disclaimer.clone())]),
            str_array(vec![json_string(ks)]),
            str_array(vec![json_string(marker_ks)]),
            str_array(vec![json_string(metadata)]),
            str_array(vec![json_string(marker_metadata)]),
            str_array(vec![json_string(units)]),
            str_array(vec![json_string(downloads)]),
            str_array(vec![json_string(supplementary)]),
        ],
    )
    .map_err(|error| schedule_error("source_scea_experiment", error))
}

fn tab_props(content: &ExperimentPageContent, tab_type: &str) -> Option<Value> {
    content
        .tabs
        .iter()
        .find(|tab| tab.tab_type == tab_type)
        .and_then(|tab| tab.props.clone())
}

fn json_string(value: Option<Value>) -> Option<String> {
    value
        .as_ref()
        .and_then(|value| serde_json::to_string(value).ok())
}

fn build_marker_batch(
    rows: &[MarkerGene],
    accession: &str,
    k: u32,
) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("accession"),
        u32("k"),
        utf8("gene_name"),
        u32("x"),
        u32("y"),
        utf8("cell_group"),
        utf8("marker_cell_group"),
        f64("expression"),
        f64("p_value"),
        utf8("expression_unit"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(vec![Some(accession.to_owned()); rows.len()]),
            u32_array(vec![Some(k); rows.len()]),
            str_array(rows.iter().map(|row| Some(row.gene_name.clone())).collect()),
            u32_array(rows.iter().map(|row| Some(row.x)).collect()),
            u32_array(rows.iter().map(|row| Some(row.y)).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.cell_group_value.clone()))
                    .collect(),
            ),
            str_array(
                rows.iter()
                    .map(|row| Some(row.cell_group_value_where_marker.clone()))
                    .collect(),
            ),
            f64_array(rows.iter().map(|row| row.value).collect()),
            f64_array(rows.iter().map(|row| row.p_value).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.expression_unit.clone()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_scea_markers", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_experiments_locally() {
        let rows = vec![Experiment {
            accession: "E-ONE-1".into(),
            description: "Lung atlas".into(),
            species: "Homo sapiens".into(),
            kingdom: "animals".into(),
            experiment_type: "Baseline".into(),
            ..Default::default()
        }];
        let filter = ExperimentFilter {
            query: Some("lung".into()),
            species: Some("homo".into()),
            ..Default::default()
        };
        assert_eq!(crate::types::filter_experiments(&rows, &filter).len(), 1);
    }

    #[test]
    fn builds_batches() {
        let experiment = Experiment::default();
        assert_eq!(
            build_experiment_batch(&[experiment]).unwrap().num_columns(),
            12
        );
        let marker = MarkerGene::default();
        assert_eq!(
            build_marker_batch(&[marker], "E-ONE-1", 3)
                .unwrap()
                .num_columns(),
            10
        );
    }
}
