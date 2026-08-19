//! Local-data integration test for MAGMA/KEGG normalization.

use std::path::Path;
use std::sync::Arc;

use arrow_array::StringArray;
use dag_core::DataBundle;
use dag_core::node::DagNode;
use dag_core::registry::NodeCtx;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::SessionContext;
use vfs::{BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, VfsManifest};

const GENE_ROOT: &str = "/mnt/data/magma/resources/genes";
const KEGG_ROOT: &str = "/mnt/disk3/kegg_scraper/data/kegg_data";

fn mounted_ctx() -> (NodeCtx, Arc<MountedObjectStore>) {
    let manifest = VfsManifest {
        backend: vec![
            BackendDefinition {
                id: "magma-local".into(),
                config: BackendConfig::local("/"),
            },
            BackendDefinition {
                id: "kegg-local".into(),
                config: BackendConfig::local("/"),
            },
        ],
        mount: vec![
            MountDefinition {
                path: "/data/magma/genes".into(),
                backend: "magma-local".into(),
                source: GENE_ROOT.into(),
                read_only: true,
            },
            MountDefinition {
                path: "/data/kegg_data".into(),
                backend: "kegg-local".into(),
                source: KEGG_ROOT.into(),
                read_only: true,
            },
        ],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let ctx = SessionContext::new();
    ctx.runtime_env().register_object_store(
        ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
        mounted.clone(),
    );
    (NodeCtx::new(ctx.runtime_env(), None), mounted)
}

#[tokio::test]
async fn normalizes_real_kegg_data_to_magma_gene_ids() {
    if !Path::new(GENE_ROOT).is_dir() || !Path::new(KEGG_ROOT).is_dir() {
        return;
    }
    let (ctx, _mounted) = mounted_ctx();
    let bundle = |id: &str, vpath: &str| DataBundle::new(id, id, vpath);
    let config = nodes_genetics::magma_kegg::MagmaKeggAlignConfig {
        organism: "hsa".into(),
        genome_id: "T01001".into(),
        gene_type: "CDS".into(),
        min_set_size: 10,
        max_set_size: 1000,
        exclude_pathways: vec!["map01100".into(), "map01110".into(), "map01120".into()],
    };
    let mut node = nodes_genetics::magma_kegg::MagmaKeggAlignNode::new(
        config,
        bundle(
            nodes_genetics::magma_kegg::GENE_LOC_BUNDLE,
            "/data/magma/genes/parquet/NCBI37.3.gene_loc.parquet",
        ),
        bundle(
            nodes_genetics::magma_kegg::KEGG_GENES_BUNDLE,
            "/data/kegg_data/entity_gene.parquet",
        ),
        bundle(
            nodes_genetics::magma_kegg::KEGG_PATHWAY_KO_BUNDLE,
            "/data/kegg_data/link_pathway_ko.parquet",
        ),
        bundle(
            nodes_genetics::magma_kegg::KEGG_PATHWAYS_BUNDLE,
            "/data/kegg_data/entity_pathway.parquet",
        ),
        bundle(
            nodes_genetics::magma_kegg::KEGG_GENOME_PATHWAYS_BUNDLE,
            "/data/kegg_data/link_genome_pathway.parquet",
        ),
    );
    let output = node
        .execute(&ctx, &[], &dag_core::dag::node_event::NodeReporter::noop())
        .await
        .unwrap();
    let df = output.dataframe(0).unwrap();
    let total_rows = df.clone().count().await.unwrap();
    assert!(total_rows > 10_000, "expected broad KEGG coverage");

    let sample = df
        .clone()
        .limit(0, Some(10))
        .unwrap()
        .collect()
        .await
        .unwrap();
    let genes = sample[0]
        .column_by_name("gene_id")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert!(
        genes
            .iter()
            .flatten()
            .all(|gene| { !gene.is_empty() && gene.parse::<u64>().is_ok() })
    );
}
