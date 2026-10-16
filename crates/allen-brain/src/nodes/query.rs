//! Query nodes over Allen Brain RMA and taxonomy index resources.

use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::AllenBrainClient;
use crate::nodes::util::{
    dataframe_output, dataframe_port, request_error, schedule_error, str_array, u32, u32_array,
    u64, u64_array, utf8,
};
use crate::types::{BrainArea, Gene, TaxonomyReference};
use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

macro_rules! impl_allen_brain_factory {
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

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct AllenBrainAreasSpec {
    #[serde(default = "default_graph_id")]
    pub graph_id: u64,
    #[serde(default)]
    pub start_row: u32,
    #[serde(default = "default_rows")]
    pub rows: u32,
}

fn default_graph_id() -> u64 {
    1
}

fn default_rows() -> u32 {
    50
}

#[derive(Clone)]
pub struct AllenBrainAreasNode {
    meta: NodePorts,
    spec: AllenBrainAreasSpec,
}

pub struct AllenBrainAreasNodeFactory;

impl_allen_brain_factory!(
    AllenBrainAreasNodeFactory,
    AllenBrainAreasNode,
    AllenBrainAreasSpec,
    "source_allen_brain_areas",
    "Queries Allen Brain atlas structures.",
    "Uses the official Allen RMA Structure model. Output contains structure \
     IDs, ontology graph, parent, acronym, name, path, depth, color, and the \
     server-side total row count. Pagination is explicit; no matrix data is \
     downloaded."
);

#[async_trait]
impl DagNode for AllenBrainAreasNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_allen_brain_areas"
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
        let page = AllenBrainClient::new()
            .areas(self.spec.graph_id, self.spec.start_row, self.spec.rows)
            .await
            .map_err(|error| request_error(error, "source_allen_brain_areas"))?;
        let batch = build_area_batch(&page.rows, page.total_rows)?;
        dataframe_output(ctx, batch, "source_allen_brain_areas")
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct AllenBrainGenesSpec {
    pub symbol: String,
    #[serde(default)]
    pub start_row: u32,
    #[serde(default = "default_rows")]
    pub rows: u32,
}

#[derive(Clone)]
pub struct AllenBrainGenesNode {
    meta: NodePorts,
    spec: AllenBrainGenesSpec,
}

pub struct AllenBrainGenesNodeFactory;

impl_allen_brain_factory!(
    AllenBrainGenesNodeFactory,
    AllenBrainGenesNode,
    AllenBrainGenesSpec,
    "source_allen_brain_genes",
    "Queries Allen Brain gene records by symbol.",
    "Uses the official Allen RMA Gene model with an exact, case-sensitive \
     acronym lookup. A symbol such as Pvalb can return organism-specific \
     records. Output includes gene, organism, Entrez, and HomoloGene IDs."
);

#[async_trait]
impl DagNode for AllenBrainGenesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_allen_brain_genes"
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
        let page = AllenBrainClient::new()
            .genes(&self.spec.symbol, self.spec.start_row, self.spec.rows)
            .await
            .map_err(|error| request_error(error, "source_allen_brain_genes"))?;
        let batch = build_gene_batch(&page.rows, page.total_rows)?;
        dataframe_output(ctx, batch, "source_allen_brain_genes")
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct AllenBrainTypesSpec {
    #[serde(default)]
    pub species: Option<String>,
    /// `10x` or `SMART-seq`.
    #[serde(default)]
    pub assay: Option<String>,
}

#[derive(Clone)]
pub struct AllenBrainTypesNode {
    meta: NodePorts,
    spec: AllenBrainTypesSpec,
}

pub struct AllenBrainTypesNodeFactory;

impl_allen_brain_factory!(
    AllenBrainTypesNodeFactory,
    AllenBrainTypesNode,
    AllenBrainTypesSpec,
    "source_allen_brain_types",
    "Lists Allen Brain cell type taxonomy references.",
    "Parses the official Allen cell type taxonomy index and emits versioned \
     H5AD reference metadata with species, assay, region, release date, and \
     download URL. Filters are metadata-only; the node never downloads H5AD \
     references."
);

#[async_trait]
impl DagNode for AllenBrainTypesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_allen_brain_types"
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
        let rows = AllenBrainClient::new()
            .taxonomies(self.spec.species.as_deref(), self.spec.assay.as_deref())
            .await
            .map_err(|error| request_error(error, "source_allen_brain_types"))?;
        let batch = build_taxonomy_batch(&rows)?;
        dataframe_output(ctx, batch, "source_allen_brain_types")
    }
}

fn build_area_batch(rows: &[BrainArea], total_rows: u64) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        u64("structure_id"),
        u64("graph_id"),
        u64("parent_structure_id"),
        utf8("acronym"),
        utf8("name"),
        utf8("structure_id_path"),
        u32("depth"),
        utf8("color_hex_triplet"),
        u64("total_rows"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            u64_array(rows.iter().map(|row| Some(row.id)).collect()),
            u64_array(rows.iter().map(|row| Some(row.graph_id)).collect()),
            u64_array(rows.iter().map(|row| row.parent_structure_id).collect()),
            str_array(rows.iter().map(|row| Some(row.acronym.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.name.clone())).collect()),
            str_array(
                rows.iter()
                    .map(|row| row.structure_id_path.clone())
                    .collect(),
            ),
            u32_array(rows.iter().map(|row| row.depth).collect()),
            str_array(
                rows.iter()
                    .map(|row| row.color_hex_triplet.clone())
                    .collect(),
            ),
            u64_array(vec![Some(total_rows); rows.len()]),
        ],
    )
    .map_err(|error| schedule_error("source_allen_brain_areas", error))
}

fn build_gene_batch(rows: &[Gene], total_rows: u64) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        u64("gene_id"),
        utf8("symbol"),
        utf8("name"),
        u64("organism_id"),
        u64("entrez_id"),
        u64("homologene_id"),
        utf8("alias_tags"),
        u64("total_rows"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            u64_array(rows.iter().map(|row| Some(row.id)).collect()),
            str_array(rows.iter().map(|row| Some(row.acronym.clone())).collect()),
            str_array(rows.iter().map(|row| Some(row.name.clone())).collect()),
            u64_array(rows.iter().map(|row| row.organism_id).collect()),
            u64_array(rows.iter().map(|row| row.entrez_id).collect()),
            u64_array(rows.iter().map(|row| row.homologene_id).collect()),
            str_array(rows.iter().map(|row| row.alias_tags.clone()).collect()),
            u64_array(vec![Some(total_rows); rows.len()]),
        ],
    )
    .map_err(|error| schedule_error("source_allen_brain_genes", error))
}

fn build_taxonomy_batch(rows: &[TaxonomyReference]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        utf8("reference_id"),
        utf8("species"),
        utf8("assay"),
        utf8("region"),
        utf8("release_date"),
        utf8("download_url"),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            str_array(rows.iter().map(|row| Some(row.filename.clone())).collect()),
            str_array(rows.iter().map(|row| row.species.clone()).collect()),
            str_array(rows.iter().map(|row| row.assay.clone()).collect()),
            str_array(rows.iter().map(|row| row.region.clone()).collect()),
            str_array(rows.iter().map(|row| row.release_date.clone()).collect()),
            str_array(
                rows.iter()
                    .map(|row| Some(row.download_url.clone()))
                    .collect(),
            ),
        ],
    )
    .map_err(|error| schedule_error("source_allen_brain_types", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_query_batches() {
        let area = BrainArea::default();
        assert_eq!(build_area_batch(&[area], 1327).unwrap().num_columns(), 9);
        let gene = Gene::default();
        assert_eq!(build_gene_batch(&[gene], 3).unwrap().num_columns(), 8);
        let taxonomy = TaxonomyReference {
            filename: "Mouse_VISp_ALM_SMART_seq_04042025.h5ad".to_owned(),
            download_url: "https://example.test/reference.h5ad".to_owned(),
            ..TaxonomyReference::default()
        };
        assert_eq!(build_taxonomy_batch(&[taxonomy]).unwrap().num_columns(), 6);
    }
}
