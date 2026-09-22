//! Normalize KEGG human pathway mappings for MAGMA gene-set analysis.
//!
//! The collected KEGG export contains direct organism-gene metadata and
//! pathway-KO mappings. Human genes are aligned to MAGMA's NCBI Gene IDs and
//! pathway membership is materialized through KEGG orthology. The output is a
//! long-format annotation table suitable for export and conversion to MAGMA's
//! row-format gene-set file.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::dag::{DagError, graph::PortOutputs, node_event::NodeReporter};
use dag_core::node::{DagNode, DataBundle, DataBundleBinding, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

const KIND: &str = "magma_kegg_align";

fn default_organism() -> String {
    "hsa".to_string()
}

fn default_genome_id() -> String {
    "T01001".to_string()
}

fn default_gene_type() -> String {
    "CDS".to_string()
}

fn default_min_set_size() -> usize {
    10
}

fn default_max_set_size() -> usize {
    1000
}

fn default_exclude_pathways() -> Vec<String> {
    ["map01100", "map01110", "map01120"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MagmaKeggAlignConfig {
    /// KEGG organism prefix. Currently tested for the human `hsa` export.
    #[serde(default = "default_organism")]
    pub organism: String,
    /// KEGG genome ID for the selected organism (`T01001` is human).
    #[serde(default = "default_genome_id")]
    pub genome_id: String,
    /// KEGG gene type retained in the MAGMA universe (`CDS` by default).
    #[serde(default = "default_gene_type")]
    pub gene_type: String,
    /// Minimum mapped genes retained per pathway.
    #[serde(default = "default_min_set_size")]
    pub min_set_size: usize,
    /// Maximum mapped genes retained per pathway.
    #[serde(default = "default_max_set_size")]
    pub max_set_size: usize,
    /// Broad reference maps excluded by default.
    #[serde(default = "default_exclude_pathways")]
    pub exclude_pathways: Vec<String>,
}

#[derive(Debug, Error)]
pub enum MagmaKeggAlignError {
    #[error("MAGMA KEGG alignment failed: {0}")]
    DataFusion(#[from] datafusion::error::DataFusionError),
}

impl dag_core::dag::NodeError for MagmaKeggAlignError {
    fn node_type(&self) -> &str {
        KIND
    }
}

fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("set_id", DataType::Utf8, false),
        Field::new("set_name", DataType::Utf8, false),
        Field::new("set_class", DataType::Utf8, true),
        Field::new("gene_id", DataType::Utf8, false),
        Field::new("n_genes", DataType::Int64, false),
        Field::new("mapping_source", DataType::Utf8, false),
    ]))
}

fn node_ports() -> NodePorts {
    NodePorts::new().add_output_port(Some(output_schema()))
}

pub struct MagmaKeggAlignNodeFactory;

pub const GENE_LOC_BUNDLE: &str = "magma-gene-loc";
pub const KEGG_GENES_BUNDLE: &str = "kegg-genes";
pub const KEGG_PATHWAY_KO_BUNDLE: &str = "kegg-pathway-ko";
pub const KEGG_PATHWAYS_BUNDLE: &str = "kegg-pathways";
pub const KEGG_GENOME_PATHWAYS_BUNDLE: &str = "kegg-genome-pathways";

fn storage_url(bundle: &DataBundle) -> String {
    format!("vfs://{}", bundle.vpath)
}

impl NodeFactory for MagmaKeggAlignNodeFactory {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn desc(&self) -> &'static str {
        "Align KEGG human pathway mappings to MAGMA NCBI Gene IDs."
    }

    fn doc(&self) -> &'static str {
        "Reads a MAGMA gene-loc Parquet table and the KEGG gene, pathway, \
        orthology-pathway, and human-pathway Parquet tables. Strips the KEGG \
        organism prefix, joins KEGG genes to the MAGMA gene universe, maps \
        genes to pathways through KEGG orthology, filters set sizes, and \
        emits a standardized long-format gene-set annotation table. The \
        output records mapping_source='orthology'."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MagmaKeggAlignConfig)
    }

    fn data_bundles(&self) -> Vec<DataBundleBinding> {
        vec![
            DataBundleBinding::new("gene_loc", GENE_LOC_BUNDLE),
            DataBundleBinding::new("kegg_genes", KEGG_GENES_BUNDLE),
            DataBundleBinding::new("kegg_pathway_ko", KEGG_PATHWAY_KO_BUNDLE),
            DataBundleBinding::new("kegg_pathways", KEGG_PATHWAYS_BUNDLE),
            DataBundleBinding::new("kegg_genome_pathways", KEGG_GENOME_PATHWAYS_BUNDLE),
        ]
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(Some(output_schema()))
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: MagmaKeggAlignConfig = serde_json::from_value(spec)?;
        if config.min_set_size == 0 {
            return Err(dag_core::registry::error::Error::Unknown(
                "min_set_size must be greater than zero".into(),
            ));
        }
        if config.max_set_size < config.min_set_size {
            return Err(dag_core::registry::error::Error::Unknown(
                "max_set_size must be greater than or equal to min_set_size".into(),
            ));
        }
        Ok(Box::new(MagmaKeggAlignNode {
            meta: node_ports(),
            config,
            gene_loc: node_ctx.bound_data_bundle("gene_loc")?.clone(),
            kegg_genes: node_ctx.bound_data_bundle("kegg_genes")?.clone(),
            kegg_pathway_ko: node_ctx.bound_data_bundle("kegg_pathway_ko")?.clone(),
            kegg_pathways: node_ctx.bound_data_bundle("kegg_pathways")?.clone(),
            kegg_genome_pathways: node_ctx.bound_data_bundle("kegg_genome_pathways")?.clone(),
        }))
    }
}

#[derive(Clone)]
pub struct MagmaKeggAlignNode {
    meta: NodePorts,
    config: MagmaKeggAlignConfig,
    gene_loc: DataBundle,
    kegg_genes: DataBundle,
    kegg_pathway_ko: DataBundle,
    kegg_pathways: DataBundle,
    kegg_genome_pathways: DataBundle,
}

impl MagmaKeggAlignNode {
    pub fn new(
        config: MagmaKeggAlignConfig,
        gene_loc: DataBundle,
        kegg_genes: DataBundle,
        kegg_pathway_ko: DataBundle,
        kegg_pathways: DataBundle,
        kegg_genome_pathways: DataBundle,
    ) -> Self {
        Self {
            meta: node_ports(),
            config,
            gene_loc,
            kegg_genes,
            kegg_pathway_ko,
            kegg_pathways,
            kegg_genome_pathways,
        }
    }

    async fn query(
        &self,
        ctx: &datafusion::prelude::SessionContext,
    ) -> Result<datafusion::dataframe::DataFrame, MagmaKeggAlignError> {
        let parquet = datafusion::prelude::ParquetReadOptions::default();
        ctx.register_parquet(
            "magma_gene_loc",
            &storage_url(&self.gene_loc),
            parquet.clone(),
        )
        .await?;
        ctx.register_parquet(
            "kegg_genes",
            &storage_url(&self.kegg_genes),
            parquet.clone(),
        )
        .await?;
        ctx.register_parquet(
            "kegg_pathway_ko",
            &storage_url(&self.kegg_pathway_ko),
            parquet.clone(),
        )
        .await?;
        ctx.register_parquet(
            "kegg_pathways",
            &storage_url(&self.kegg_pathways),
            parquet.clone(),
        )
        .await?;
        ctx.register_parquet(
            "kegg_genome_pathways",
            &storage_url(&self.kegg_genome_pathways),
            parquet,
        )
        .await?;

        let organism = escape_sql_string(&self.config.organism);
        let genome_id = escape_sql_string(&self.config.genome_id);
        let gene_type = escape_sql_string(&self.config.gene_type);
        let excludes = if self.config.exclude_pathways.is_empty() {
            "''".to_string()
        } else {
            self.config
                .exclude_pathways
                .iter()
                .map(|id| escape_sql_string(id))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let sql = format!(
            r#"
            WITH universe AS (
                SELECT DISTINCT gene_id
                FROM magma_gene_loc
            ),
            kegg_gene AS (
                SELECT
                    regexp_replace(g.id, '^' || {organism} || ':', '') AS gene_id,
                    g.orthology
                FROM kegg_genes AS g
                WHERE g.organism = {organism}
                  AND g.gene_type = {gene_type}
                  AND g.orthology <> ''
            ),
            human_pathways AS (
                SELECT
                    pathway_id AS set_id,
                    'map' || substr(pathway_id, 4) AS map_pathway_id
                FROM kegg_genome_pathways
                WHERE genome_id = {genome_id}
            ),
            aligned AS (
                SELECT DISTINCT
                    hp.set_id,
                    p.name AS set_name,
                    nullif(p.class_str, '') AS set_class,
                    u.gene_id
                FROM kegg_gene AS g
                JOIN universe AS u ON u.gene_id = g.gene_id
                JOIN kegg_pathway_ko AS pk ON pk.ko_id = g.orthology
                JOIN human_pathways AS hp ON hp.map_pathway_id = pk.pathway_id
                JOIN kegg_pathways AS p ON p.id = hp.map_pathway_id
                WHERE pk.pathway_id LIKE 'map%'
                  AND hp.map_pathway_id NOT IN ({excludes})
            ),
            sized AS (
                SELECT
                    a.set_id,
                    m.set_name,
                    m.set_class,
                    a.gene_id,
                    a.n_genes
                FROM (
                    SELECT
                        set_id,
                        gene_id,
                        count(*) OVER (PARTITION BY set_id) AS n_genes
                    FROM aligned
                ) AS a
                JOIN (
                    SELECT
                        set_id,
                        max(set_name) AS set_name,
                        max(set_class) AS set_class
                    FROM aligned
                    GROUP BY set_id
                ) AS m USING (set_id)
            )
            SELECT
                arrow_cast(set_id, 'Utf8') AS set_id,
                arrow_cast(set_name, 'Utf8') AS set_name,
                arrow_cast(set_class, 'Utf8') AS set_class,
                arrow_cast(gene_id, 'Utf8') AS gene_id,
                n_genes,
                arrow_cast('orthology', 'Utf8') AS mapping_source
            FROM sized
            WHERE n_genes BETWEEN {min} AND {max}
            ORDER BY set_id, gene_id
            "#,
            min = self.config.min_set_size,
            max = self.config.max_set_size,
        );
        Ok(ctx.sql(&sql).await?)
    }
}

fn escape_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[async_trait]
impl DagNode for MagmaKeggAlignNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let df = self.query(&node_ctx.session()).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    fn bundle(id: &str, vpath: &str) -> DataBundle {
        DataBundle::new(id, id, vpath)
    }

    use super::*;
    use arrow_array::RecordBatch;
    use arrow_array::{Int64Array, StringArray, UInt64Array};
    use datafusion::dataframe::DataFrameWriteOptions;
    use datafusion::prelude::SessionContext;

    async fn write_batch(
        ctx: &SessionContext,
        name: &str,
        schema: SchemaRef,
        columns: Vec<Arc<dyn arrow_array::Array>>,
        path: &std::path::Path,
    ) {
        let batch = RecordBatch::try_new(schema, columns).unwrap();
        let df = ctx.read_batch(batch).unwrap();
        df.write_parquet(
            &path.to_string_lossy(),
            DataFrameWriteOptions::new().with_single_file_output(true),
            None::<datafusion::config::TableParquetOptions>,
        )
        .await
        .unwrap();
        ctx.deregister_table(name).unwrap();
    }

    #[tokio::test]
    async fn aligns_kegg_ids_to_magma_gene_universe() {
        let root = tempfile::tempdir().unwrap();
        let ctx = SessionContext::new();

        let gene_schema = Arc::new(Schema::new(vec![
            Field::new("gene_id", DataType::Utf8, false),
            Field::new("chromosome", DataType::Utf8, false),
            Field::new("start", DataType::UInt64, false),
            Field::new("end", DataType::UInt64, false),
            Field::new("strand", DataType::Utf8, false),
            Field::new("symbol", DataType::Utf8, true),
        ]));
        write_batch(
            &ctx,
            "gene_loc",
            gene_schema,
            vec![
                Arc::new(StringArray::from(vec!["100", "101", "900"])),
                Arc::new(StringArray::from(vec!["1", "1", "2"])),
                Arc::new(UInt64Array::from(vec![1, 20, 30])),
                Arc::new(UInt64Array::from(vec![10, 30, 40])),
                Arc::new(StringArray::from(vec!["+", "+", "-"])),
                Arc::new(StringArray::from(vec![Some("A"), Some("B"), None])),
            ],
            &root.path().join("gene_loc.parquet"),
        )
        .await;

        let kegg_gene_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("gene_type", DataType::Utf8, false),
            Field::new("organism", DataType::Utf8, false),
            Field::new("orthology", DataType::Utf8, false),
        ]));
        write_batch(
            &ctx,
            "kegg_gene",
            kegg_gene_schema,
            vec![
                Arc::new(StringArray::from(vec!["hsa:100", "hsa:101", "hsa:900"])),
                Arc::new(StringArray::from(vec!["CDS", "CDS", "ncRNA"])),
                Arc::new(StringArray::from(vec!["hsa", "hsa", "hsa"])),
                Arc::new(StringArray::from(vec!["K1", "K2", "K3"])),
            ],
            &root.path().join("entity_gene.parquet"),
        )
        .await;

        let link_schema = Arc::new(Schema::new(vec![
            Field::new("pathway_id", DataType::Utf8, false),
            Field::new("ko_id", DataType::Utf8, false),
        ]));
        write_batch(
            &ctx,
            "pathway_ko",
            link_schema,
            vec![
                Arc::new(StringArray::from(vec!["map00010", "map00010", "ko00010"])),
                Arc::new(StringArray::from(vec!["K1", "K2", "K1"])),
            ],
            &root.path().join("link_pathway_ko.parquet"),
        )
        .await;

        let pathway_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("class_str", DataType::Utf8, true),
        ]));
        write_batch(
            &ctx,
            "pathways",
            pathway_schema,
            vec![
                Arc::new(StringArray::from(vec!["map00010"])),
                Arc::new(StringArray::from(vec!["Glycolysis"])),
                Arc::new(StringArray::from(vec![Some("Metabolism")])),
            ],
            &root.path().join("entity_pathway.parquet"),
        )
        .await;

        let genome_path_schema = Arc::new(Schema::new(vec![
            Field::new("genome_id", DataType::Utf8, false),
            Field::new("pathway_id", DataType::Utf8, false),
        ]));
        write_batch(
            &ctx,
            "genome_paths",
            genome_path_schema,
            vec![
                Arc::new(StringArray::from(vec!["T01001", "T99999"])),
                Arc::new(StringArray::from(vec!["hsa00010", "hsa00010"])),
            ],
            &root.path().join("link_genome_pathway.parquet"),
        )
        .await;

        let manifest = vfs::VfsManifest {
            backend: vec![vfs::BackendDefinition {
                id: "test".into(),
                config: vfs::BackendConfig::local("/"),
            }],
            mount: vec![vfs::MountDefinition {
                path: "/data/kegg-test".into(),
                backend: "test".into(),
                source: root.path().to_string_lossy().to_string(),
                read_only: true,
            }],
        };
        let mounted = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
        ctx.runtime_env().register_object_store(
            datafusion::execution::object_store::ObjectStoreUrl::parse("vfs://")
                .unwrap()
                .as_ref(),
            mounted,
        );
        let node_ctx = NodeCtx::new(ctx.runtime_env(), None);

        let mut node = MagmaKeggAlignNode::new(
            MagmaKeggAlignConfig {
                organism: "hsa".into(),
                genome_id: "T01001".into(),
                gene_type: "CDS".into(),
                min_set_size: 1,
                max_set_size: 10,
                exclude_pathways: vec![],
            },
            bundle(GENE_LOC_BUNDLE, "/data/kegg-test/gene_loc.parquet"),
            bundle(KEGG_GENES_BUNDLE, "/data/kegg-test/entity_gene.parquet"),
            bundle(
                KEGG_PATHWAY_KO_BUNDLE,
                "/data/kegg-test/link_pathway_ko.parquet",
            ),
            bundle(
                KEGG_PATHWAYS_BUNDLE,
                "/data/kegg-test/entity_pathway.parquet",
            ),
            bundle(
                KEGG_GENOME_PATHWAYS_BUNDLE,
                "/data/kegg-test/link_genome_pathway.parquet",
            ),
        );
        let output = node
            .execute(
                &node_ctx,
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let df = output.dataframe(0).unwrap();
        let results = df.clone().collect().await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].num_rows(), 2);

        let set_ids = results[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let genes = results[0]
            .column(3)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let sizes = results[0]
            .column(4)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(set_ids.value(0), "hsa00010");
        assert_eq!(genes.value(0), "100");
        assert_eq!(genes.value(1), "101");
        assert_eq!(sizes.value(0), 2);
    }
}
