//! Reproduction probe for the reported `univariate_mixer` chrom dependency.
//!
//! Symptom (from the TUI agent): feeding the node a `chrom` column makes it
//! succeed; omitting `chrom` fails with "no SNPs overlap" — even though the
//! node's documented input schema is only `{Z, N, rsid}` and the node body
//! never references `s.chrom`.
//!
//! This probe drives the node **directly** (no upstream pipeline), feeding the
//! *same* GWAS sumstats twice with only the `chrom` projection toggled. That
//! isolates whether the chrom dependency lives in the node itself or upstream.
//!
//! Expected outcomes:
//! - If both variants succeed with **identical** fit numbers → the node is
//!   chrom-independent; the production bug is upstream (the upstream node
//!   silently emits empty/bad sumstats without chrom).
//! - If the no-`chrom` variant fails with "no SNPs overlap" while the `chrom`
//!   variant succeeds → there IS a hidden node-side / DataFusion-view
//!   dependency on the column set, and the node contract is wrong.
//!
//! Manual run:
//! ```sh
//! cargo test -p data-engine --test univariate_mixer_chrom_repro -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Instant;

use datalake::Datalake;

use data_engine::node_registry::registry::NodeCtx;
use data_engine::nodes::meta::{DagNode, NodeInput};
use data_engine::nodes::univariate_mixer::{UnivariateMixerNode, UnivariateMixerNodeSpec};

/// Base SELECT computing Z = β/SE and N = sample_size from chr22 GWAS.
/// `{chrom_proj}` is either `` (omit chrom) or `chrom,` (include it).
const SUMSTATS_TEMPLATE: &str = r#"SELECT rsid, {chrom_proj} effect_size / std_error AS "Z", sample_size AS "N"
    FROM iceberg.gwas.bmi_ieu_a_2
    WHERE chrom = '22'
      AND std_error > 0
      AND effect_size IS NOT NULL"#;

/// Build the node and feed `sumstats` as input port 0. Returns the fit row
/// collected from output port 0, or the DagError message.
async fn run_node(
    node_ctx: &NodeCtx,
    sumstats: datafusion::dataframe::DataFrame,
    label: &str,
) -> Result<String, String> {
    let spec = UnivariateMixerNodeSpec {
        chromosomes: vec![22],
        diffevo_repeats: 2, // speed: this is a repro, not a production fit
        r2_min: 0.05,
        seed: 123,
        extract_enabled: true,
        extract_maf: 0.05,
        extract_subset: 2_000_000,
        extract_r2: 0.8,
    };
    let mut node = UnivariateMixerNode::new(spec);

    // Print the input schema so we can SEE which columns the node received.
    let fields: Vec<String> = sumstats
        .schema()
        .fields()
        .iter()
        .map(|f| format!("{}:{:?}", f.name(), f.data_type()))
        .collect();
    println!("[{label}] input sumstats schema: [{}]", fields.join(", "));

    let input = NodeInput {
        port: 0u8,
        data: sumstats,
    };

    let started = Instant::now();
    let outputs = node
        .execute(
            node_ctx,
            &[input],
            &data_engine::dag::node_event::NodeReporter::noop(),
        )
        .await
        .map_err(|e| {
            format!(
                "execute failed after {:.1}s: {e}",
                started.elapsed().as_secs_f64()
            )
        })?;
    let elapsed = started.elapsed().as_secs_f64();

    let out_df = outputs
        .get(&0u8)
        .cloned()
        .ok_or_else(|| "no output port 0".to_string())?;
    let batches = out_df
        .collect()
        .await
        .map_err(|e| format!("collect: {e}"))?;

    let mut rows = Vec::new();
    for b in &batches {
        let row: Vec<String> = (0..b.num_columns())
            .map(|c| format!("{:?}", b.column(c)))
            .collect();
        rows.push(row.join(" | "));
    }
    println!("[{label}] OK in {elapsed:.1}s → {} row(s)", rows.len());
    Ok(rows.join("\n"))
}

#[tokio::test]
#[ignore]
async fn repro_chrom_presence_changes_result() {
    let dk = Arc::new(Datalake::new());
    let ctx = dk.get_ctx().await.expect("无法连 Iceberg 数据湖");
    let node_ctx = NodeCtx {
        runtime_env: ctx.runtime_env(),
        iceberg_catalog: Some(Arc::new(
            dk.get_provider().await.expect("datalake provider"),
        )),
        datalake: dk.clone(),
        opendal: None,
    };

    // --- variant A: NO chrom column (matches the documented input schema) ---
    let sql_a = SUMSTATS_TEMPLATE.replace("{chrom_proj}", "");
    let sumstats_a = ctx.sql(&sql_a).await.expect("sumstats A SQL");
    let n_a = sumstats_a.clone().count().await.expect("count A");
    println!("=== Variant A (no chrom): {n_a} input rows ===");

    // --- variant B: WITH chrom column (the "fix" the agent found) ---
    let sql_b = SUMSTATS_TEMPLATE.replace("{chrom_proj}", "chrom,");
    let sumstats_b = ctx.sql(&sql_b).await.expect("sumstats B SQL");
    let n_b = sumstats_b.clone().count().await.expect("count B");
    println!("=== Variant B (with chrom): {n_b} input rows ===");

    // Sanity: both variants must carry the same rows, just ±chrom.
    assert_eq!(n_a, n_b, "two variants should have identical row counts");

    let res_a = run_node(&node_ctx, sumstats_a, "A no-chrom").await;
    let res_b = run_node(&node_ctx, sumstats_b, "B with-chrom").await;

    println!("\n========== VERDICT ==========");
    match (&res_a, &res_b) {
        (Ok(a), Ok(b)) => {
            println!("Both variants succeeded.");
            if a == b {
                println!("→ IDENTICAL results. Node is chrom-INDEPENDENT; bug is upstream.");
            } else {
                println!("→ DIFFERENT results despite identical data!");
                println!("A:\n{a}");
                println!("B:\n{b}");
                println!("→ Node has a hidden dependency on the chrom column.");
            }
        }
        (Err(e), Ok(_)) => {
            println!("no-chrom FAILED, with-chrom SUCCEEDED:");
            println!("  A error: {e}");
            println!("→ Reproduces the reported bug at the NODE level (not just upstream).");
        }
        (Ok(_), Err(e)) => {
            println!("with-chrom FAILED (unexpected): {e}");
        }
        (Err(ea), Err(eb)) => {
            println!("Both FAILED:");
            println!("  A (no chrom):   {ea}");
            println!("  B (with chrom): {eb}");
        }
    }
    println!("=============================");
}
