use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use dag_core::BundleRegistry;
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::NodeInput;
use dag_core::registry::NodeCtx;
use datafusion::prelude::SessionContext;

fn batch(columns: Vec<(&str, Vec<String>)>) -> RecordBatch {
    let fields = columns
        .iter()
        .map(|(name, _)| Field::new(*name, DataType::Utf8, false))
        .collect::<Vec<_>>();
    let arrays = columns
        .into_iter()
        .map(|(_, values)| Arc::new(StringArray::from(values)) as Arc<dyn arrow_array::Array>)
        .collect::<Vec<_>>();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
}

fn marker_rows() -> (BTreeSet<(String, String)>, Vec<Vec<String>>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/als_cns_cell_markers/package-v1/payload/markers.tsv");
    let source = std::fs::read_to_string(path).unwrap();
    let mut rows = source.lines();
    let header: Vec<_> = rows.next().unwrap().split('\t').collect();
    let gene_i = header
        .iter()
        .position(|value| *value == "gene_symbol")
        .unwrap();
    let set_i = header.iter().position(|value| *value == "set_id").unwrap();
    let evidence_i = header
        .iter()
        .position(|value| *value == "evidence")
        .unwrap();
    let mut annotation = BTreeSet::new();
    for line in rows {
        let values: Vec<_> = line.split('\t').collect();
        if values[evidence_i] == "canonical" {
            annotation.insert((values[gene_i].to_string(), values[set_i].to_string()));
        }
    }

    let metadata_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/als_cns_cell_markers/package-v1/payload/metadata.tsv");
    let metadata_source = std::fs::read_to_string(metadata_path).unwrap();
    let mut metadata_rows = metadata_source.lines();
    let metadata_header: Vec<_> = metadata_rows.next().unwrap().split('\t').collect();
    let id_i = metadata_header
        .iter()
        .position(|value| *value == "set_id")
        .unwrap();
    let name_i = metadata_header
        .iter()
        .position(|value| *value == "set_name")
        .unwrap();
    let class_i = metadata_header
        .iter()
        .position(|value| *value == "set_class")
        .unwrap();
    let mut metadata = Vec::new();
    for line in metadata_rows {
        let values: Vec<_> = line.split('\t').collect();
        metadata.push(vec![
            values[id_i].to_string(),
            values[name_i].to_string(),
            values[class_i].to_string(),
        ]);
    }
    (annotation, metadata)
}

#[tokio::test]
async fn als_cns_marker_bundle_feeds_enrichment_ora() {
    let ctx = NodeCtx::new(SessionContext::new().runtime_env(), None);
    let registry = data_engine::default_registry::build_default_registry(
        SessionContext::new().runtime_env(),
        None,
        Arc::new(BundleRegistry::new()),
    );
    let mut node = registry
        .build_node(
            "enrichment_ora",
            serde_json::json!({
                "gene_col": "gene_symbol",
                "min_set_size": 1,
                "max_set_size": 1000,
                "min_hits": 1
            }),
        )
        .unwrap();

    let query = batch(vec![(
        "gene_symbol",
        vec![
            "GFAP", "AQP4", "S100B", "ALDH1L1", "SLC1A3", "VIM", "GLUL", "SOX9", "GJA1", "CLU",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    )]);
    let (annotation, metadata) = marker_rows();
    let annotation = batch(vec![
        (
            "gene_id",
            annotation.iter().map(|(gene, _)| gene.clone()).collect(),
        ),
        (
            "set_id",
            annotation.iter().map(|(_, set)| set.clone()).collect(),
        ),
    ]);
    let metadata = batch(vec![
        (
            "set_id",
            metadata.iter().map(|row| row[0].clone()).collect(),
        ),
        (
            "set_name",
            metadata.iter().map(|row| row[1].clone()).collect(),
        ),
        (
            "set_class",
            metadata.iter().map(|row| row[2].clone()).collect(),
        ),
    ]);

    let inputs = vec![
        NodeInput::new_dataframe(0, ctx.session().read_batch(query).unwrap()),
        NodeInput::new_dataframe(1, ctx.session().read_batch(annotation).unwrap()),
        NodeInput::new_dataframe(2, ctx.session().read_batch(metadata).unwrap()),
    ];
    let outputs = node
        .execute(&ctx, &inputs, &NodeReporter::noop())
        .await
        .unwrap();
    let result = outputs.get(&0).unwrap().as_dataframe().unwrap();
    let schema = result.schema();
    assert_eq!(
        schema.field_with_name(None, "set_id").unwrap().data_type(),
        &DataType::Utf8
    );
    assert_eq!(
        schema.field_with_name(None, "p_adj").unwrap().data_type(),
        &DataType::Float64
    );
    let rows = result.clone().collect().await.unwrap();
    assert_eq!(rows.iter().map(RecordBatch::num_rows).sum::<usize>(), 3);

    let set_ids = rows[0]
        .column_by_name("set_id")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let hits = rows[0]
        .column_by_name("hit_n")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    let p_adj = rows[0]
        .column_by_name("p_adj")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    let astrocyte = (0..set_ids.len())
        .find(|index| set_ids.value(*index) == "ALS_CNS_ASTROCYTE")
        .unwrap();
    assert_eq!(hits.value(astrocyte), 10);
    assert!(p_adj.value(astrocyte) <= 0.05);
}
