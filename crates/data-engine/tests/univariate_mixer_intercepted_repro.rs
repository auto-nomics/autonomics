//! Decisive repro: feed the **exact intercepted upstream data**
//! (`iceberg.gwas.ieu_a_2_chr22` — columns `Z, N, rsid`, NO `chrom`, ~32k
//! overlap with af.eur_af chr22) straight into `UnivariateMixerNode`.
//!
//! If the node succeeds → the "no chrom ⇒ no SNPs overlap" symptom is NOT a
//! node-level data dependency; it's something in the TUI harness/pipeline.
//! If it fails with "no SNPs overlap" → genuine node-side bug.
//!
//! ```sh
//! cargo test -p data-engine --test univariate_mixer_intercepted_repro -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Instant;

use datalake::Datalake;

use data_engine::nodes::meta::{DagNode, NodeInput};
use data_engine::nodes::univariate_mixer::{
    UnivariateMixerNode, UnivariateMixerNodeSpec, WeightingMode,
};

#[tokio::test]
#[ignore]
async fn node_runs_on_intercepted_nochrom_input() {
    let dk = Arc::new(Datalake::new());
    let ctx = dk.get_ctx().await.expect("无法连 Iceberg 数据湖");

    // The exact DataFrame the TUI pipeline handed the node.
    let sumstats = ctx
        .sql("SELECT \"Z\", \"N\", rsid FROM iceberg.gwas.ieu_a_2_chr22")
        .await
        .expect("read intercepted table");
    let n_in = sumstats.clone().count().await.expect("count");
    println!(
        "intercepted input: {n_in} rows, schema = {:?}",
        sumstats
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect::<Vec<_>>()
    );

    let spec = UnivariateMixerNodeSpec {
        chromosomes: vec![22],
        diffevo_repeats: 2,
        r2_min: 0.05,
        weighting: WeightingMode::Randprune, // preserve prior randprune behavior
        randprune_n: 64,
        randprune_r2: 0.1,
        seed: 123,
        extract_enabled: true,
        extract_maf: 0.05,
        extract_subset: 2_000_000,
        extract_r2: 0.8,
    };
    let mut node = UnivariateMixerNode::new(ctx.clone(), spec);

    let input = NodeInput {
        port: 0u8,
        data: sumstats,
    };
    let started = Instant::now();
    let outputs = node
        .execute(
            &[input],
            &data_engine::dag::node_event::NodeReporter::noop(),
        )
        .await;
    let elapsed = started.elapsed().as_secs_f64();

    match outputs {
        Ok(out) => {
            let df = out.get(&0u8).cloned().expect("output port 0");
            let batches = df.collect().await.expect("collect");
            let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
            println!("✅ execute SUCCEEDED in {elapsed:.1}s — {rows} output row(s)");
            for b in &batches {
                println!("{b:?}");
            }
        }
        Err(e) => {
            println!("❌ execute FAILED in {elapsed:.1}s: {e}");
        }
    }
}
