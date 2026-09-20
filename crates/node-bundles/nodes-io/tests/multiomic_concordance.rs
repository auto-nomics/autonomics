use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use dag_core::NodeFactory;
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::NodeInput;
use dag_core::registry::NodeCtx;
use nodes_io::multiomic_concordance::MultiomicConcordanceNodeFactory;
use vfs::OpendalFileStorage;

fn batch(symbols: [&str; 3], effects: [f64; 3], pvalues: [f64; 3]) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("gene_symbol", DataType::Utf8, false),
            Field::new("log_fc", DataType::Float64, false),
            Field::new("pvalue", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(Vec::from(symbols))),
            Arc::new(Float64Array::from(Vec::from(effects))),
            Arc::new(Float64Array::from(Vec::from(pvalues))),
        ],
    )
    .unwrap()
}

fn protein_batch(ids: &[&str], effects: &[f64], neg_log10_p: &[f64]) -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("protein_id", DataType::Utf8, false),
            Field::new("protein_log2_fc", DataType::Float64, false),
            Field::new("neg_log10_p", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(
                ids.iter()
                    .map(|value| (*value).to_string())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(effects.to_vec())),
            Arc::new(Float64Array::from(neg_log10_p.to_vec())),
        ],
    )
    .unwrap()
}

fn mapping_batch() -> RecordBatch {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("input_id", DataType::Utf8, false),
            Field::new("gene_symbol", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(StringArray::from(vec!["P1", "P2", "P3"])),
            Arc::new(StringArray::from(vec!["A", "B", "C"])),
        ],
    )
    .unwrap()
}

#[tokio::test]
async fn joins_three_modalities_and_publishes_ranked_reports() {
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(OpendalFileStorage::new(root.path()));
    let ctx = NodeCtx::new(
        datafusion::prelude::SessionContext::new().runtime_env(),
        Some(storage),
    );
    let mut node = MultiomicConcordanceNodeFactory
        .build(
            serde_json::json!({
                "tissue_protein": {
                    "gene_col": "protein_id",
                    "effect_col": "protein_log2_fc",
                    "pvalue_col": "neg_log10_p",
                    "pvalue_transform": "negative_log10"
                },
                "per_input_p_cutoff": 0.05,
                "require_sign_consistency": true,
                "artifact_prefix": "/artifacts/multiomic-concordance-test"
            }),
            ctx.clone(),
        )
        .unwrap();

    let rna = batch(["A", "B", "C"], [1.0, -2.0, 1.5], [0.01, 0.01, 0.01]);
    let tissue = protein_batch(&["P1;P2"], &[0.5], &[1.7]);
    let csf = batch(["A", "B", "C"], [0.2, -1.5, -0.1], [0.03, 0.03, 0.03]);
    let inputs = [rna, tissue, csf, mapping_batch()]
        .into_iter()
        .enumerate()
        .map(|(port, batch)| {
            NodeInput::new_dataframe(port as u8, ctx.session().read_batch(batch).unwrap())
        })
        .collect::<Vec<_>>();

    let outputs = node
        .execute(&ctx, &inputs, &NodeReporter::noop())
        .await
        .unwrap();
    let expected = [
        "ranked_candidates.tsv",
        "filter_counts.tsv",
        "matched_symbols.tsv",
        "run_report.json",
    ];
    for (index, name) in expected.into_iter().enumerate() {
        let file = outputs.get(&(index as u8)).unwrap().as_file().unwrap();
        assert_eq!(
            file.path,
            format!("vfs:///artifacts/multiomic-concordance-test/{name}")
        );
        assert!(
            file.fingerprint
                .as_ref()
                .and_then(|fingerprint| fingerprint.content_hash.as_ref())
                .is_some()
        );
    }

    async fn read(ctx: &NodeCtx, file: &dag_core::value::FileRef) -> Vec<u8> {
        let storage = ctx.opendal.as_ref().unwrap();
        let path = file.path.strip_prefix("vfs://").unwrap();
        storage
            .resolve(path)
            .read(&storage.resolve_path(path))
            .await
            .unwrap()
            .to_vec()
    }

    let ranked = outputs.get(&0).unwrap().as_file().unwrap().clone();
    let ranked = read(&ctx, &ranked).await;
    let ranked_text = String::from_utf8(ranked).unwrap();
    assert!(ranked_text.starts_with("gene_symbol\trank\tlog_fc_rna\t"));
    assert!(ranked_text.contains("A\t1\t"));
    assert_eq!(ranked_text.lines().count(), 2);

    let counts = outputs.get(&1).unwrap().as_file().unwrap().clone();
    let counts = read(&ctx, &counts).await;
    let counts_text = String::from_utf8(counts).unwrap();
    assert!(counts_text.contains("input_tissue_protein\t1"));
    assert!(counts_text.contains("exploded_symbols_tissue_protein\t2"));
    assert!(counts_text.contains("direction_consistent\t1"));

    let report = outputs.get(&3).unwrap().as_file().unwrap().clone();
    let report = read(&ctx, &report).await;
    let report: serde_json::Value = serde_json::from_slice(&report).unwrap();
    assert_eq!(report["node"], "multiomic_concordance");
    assert_eq!(report["analysis"]["id_mapping_entries"], 3);
    assert_eq!(
        report["analysis"]["columns"]["tissue_protein"]["pvalue_transform"],
        "negative_log10"
    );
    assert_eq!(report["analysis"]["per_input_p_cutoff"], 0.05);
    assert_eq!(report["analysis"]["require_sign_consistency"], true);
}
