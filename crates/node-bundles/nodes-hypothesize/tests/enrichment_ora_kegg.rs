//! Optional full-scale KEGG validation.
//!
//! Set `KEGG_DATA_DIR` to the directory containing the KEGG parquet tables to
//! run this test. It is a no-op when that local data asset is unavailable.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray, StringViewArray};
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::{ParquetReadOptions, SessionContext};
use nodes_hypothesize::enrichment_ora::EnrichmentOraNodeFactory;

fn string_value(batch: &RecordBatch, column: usize, row: usize) -> String {
    let array = batch.column(column).as_any();
    if let Some(values) = array.downcast_ref::<StringArray>() {
        return values.value(row).to_string();
    }
    if let Some(values) = array.downcast_ref::<StringViewArray>() {
        return values.value(row).to_string();
    }
    panic!("output column {column} is not a string array")
}

#[tokio::test]
async fn kegg_ora_reproduces_validated_hypergeometric_result() {
    let Some(data_dir) = std::env::var_os("KEGG_DATA_DIR").map(PathBuf::from) else {
        return;
    };

    let ctx = SessionContext::new();
    for table in ["entity_gene", "link_pathway_ko", "entity_pathway"] {
        let path = data_dir.join(format!("{table}.parquet"));
        ctx.register_parquet(
            table,
            path.as_os_str().to_string_lossy(),
            ParquetReadOptions::default(),
        )
        .await
        .unwrap();
    }

    let membership_sql = r#"
        SELECT DISTINCT g.gene_id, l.pathway_id AS set_id
        FROM (
            SELECT trim(split_part(symbol, ',', 1)) AS gene_id, orthology AS ko_id
            FROM entity_gene
            WHERE organism = 'hsa'
              AND gene_type = 'CDS'
              AND symbol <> ''
              AND orthology <> ''
        ) AS g
        JOIN link_pathway_ko AS l
          ON g.ko_id = l.ko_id
        WHERE l.pathway_id LIKE 'map%'
    "#;
    let mapping = ctx.sql(membership_sql).await.unwrap();
    ctx.register_table("membership", mapping.clone().into_view())
        .unwrap();
    let metadata = ctx
        .sql(
            r#"
            SELECT id AS set_id, name AS set_name, nullif(class_str, '') AS set_class
            FROM entity_pathway
            WHERE id LIKE 'map%'
            "#,
        )
        .await
        .unwrap();
    let query = ctx
        .sql(
            r#"
            WITH target AS (
                SELECT DISTINCT gene_id FROM membership WHERE set_id = 'map00010'
            )
            SELECT gene_id FROM target
            UNION ALL
            SELECT gene_id FROM (
                SELECT m.gene_id
                FROM membership AS m
                LEFT JOIN target AS t USING (gene_id)
                WHERE t.gene_id IS NULL
                GROUP BY m.gene_id
                ORDER BY m.gene_id
                LIMIT 40
            ) AS controls
            "#,
        )
        .await
        .unwrap();

    let node_ctx = NodeCtx {
        runtime_env: ctx.runtime_env(),
        opendal: None,
        global_sem: None,
    };
    let spec = serde_json::json!({
        "gene_col": "gene_id",
        "min_set_size": 5,
        "max_set_size": 500,
        "min_hits": 2,
        "exclude_sets": ["map01100", "map01110", "map01120"],
        "background_mode": "annotation_all",
        "alternative": "greater",
        "test": "hypergeometric",
        "correction": "BH",
        "alpha": 0.05
    });
    let mut node = EnrichmentOraNodeFactory {}
        .build(spec, node_ctx.clone())
        .unwrap();
    let inputs = vec![
        NodeInput {
            port: 0,
            data: query,
        },
        NodeInput {
            port: 1,
            data: mapping,
        },
        NodeInput {
            port: 2,
            data: metadata,
        },
    ];

    let started = Instant::now();
    let outputs = node
        .execute(&node_ctx, &inputs, &NodeReporter::noop())
        .await
        .unwrap();
    let batches = outputs.get(&0).unwrap().clone().collect().await.unwrap();
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    assert!(batch.num_rows() >= 70);
    assert_eq!(string_value(batch, 0, 0), "map00010");

    let universe_n = batch
        .column(3)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap()
        .value(0);
    let query_n = batch
        .column(4)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap()
        .value(0);
    let hit_n = batch
        .column(5)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap()
        .value(0);
    let set_n = batch
        .column(6)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap()
        .value(0);
    let p_raw = batch
        .column(9)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap()
        .value(0);
    let p_adj = batch
        .column(10)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap()
        .value(0);
    let hit_genes = string_value(batch, 12, 0);

    assert_eq!((universe_n, query_n, hit_n, set_n), (8645, 107, 67, 67));
    let expected_raw = 3.350684888061157e-140;
    assert!(
        (p_raw - expected_raw).abs() / expected_raw < 1e-10,
        "p_raw={p_raw:.17e}"
    );
    let expected_adj = 2.412493119404033e-138;
    assert!(
        (p_adj - expected_adj).abs() / expected_adj < 1e-10,
        "p_adj={p_adj}"
    );
    assert_eq!(hit_genes.split(',').count(), 67);
    assert!(started.elapsed().as_secs() < 30);
}
