//! `UnivariateMixerNode` 端到端集成测试：用真实 Iceberg GWAS 数据驱动 DAG 节点本身。
//!
//! 与 `bio_crates/mixer/tests/test_univariate_mixer_e2e.rs`（在 mixer crate 内复刻
//! 组装逻辑）互补——本测试直接构造节点、喂入上游 sumstats DataFrame、调用
//! `DagNode::execute`，验证生产节点路径能正常跑通。
//!
//! 上游 sumstats：从 `iceberg.gwas.bmi_ieu_a_2` 用 SQL 算出 Z = effect_size/std_error、
//! N = sample_size、rsid（节点输入端口要求列名 `Z`/`N`/`rsid`）。
//!
//! 手动运行：
//! ```sh
//! cargo test -p data-engine --test univariate_mixer_node_e2e -- --ignored --nocapture
//! ```

use std::sync::Arc;

use datalake::Datalake;

use data_engine::nodes::meta::{DagNode, NodeInput};
use data_engine::nodes::univariate_mixer::{
    UnivariateMixerNode, UnivariateMixerNodeSpec, WeightingMode,
};

#[tokio::test]
#[ignore]
async fn univariate_mixer_node_runs_on_iceberg_gwas() {
    let dk = Arc::new(Datalake::new());

    // 1. 上游 sumstats DataFrame：Z = β/SE，N = sample_size。列名必须为 Z/N/rsid。
    let ctx = dk.get_ctx().await.expect("无法连 Iceberg 数据湖");
    let sumstats = ctx
        .sql(
            r#"SELECT rsid, effect_size / std_error AS "Z", sample_size AS "N"
               FROM iceberg.gwas.bmi_ieu_a_2
               WHERE chrom = '22'
                 AND std_error > 0
                 AND effect_size IS NOT NULL"#,
        )
        .await
        .expect("sumstats SQL");

    let n_in = sumstats.clone().count().await.expect("count");
    println!("上游 sumstats 行数: {n_in}");
    assert!(n_in > 1000);

    // 2. 构造节点（chr22，diffevo_repeats=2 加速）。
    let spec = UnivariateMixerNodeSpec {
        chromosomes: vec![22],
        diffevo_repeats: 2,
        r2_min: 0.05,
        weighting: WeightingMode::LdScore,
        randprune_n: 64,
        randprune_r2: 0.1,
        seed: 123,
        extract_enabled: true,
        extract_maf: 0.05,
        extract_subset: 2_000_000,
        extract_r2: 0.8,
    };
    let mut node = UnivariateMixerNode::new(ctx.clone(), spec);

    // 3. 喂入并执行。
    let input = NodeInput {
        port: 0u8,
        data: sumstats,
    };
    let outputs = node.execute(&[input]).await.expect("节点 execute 失败");

    // 4. 收集输出（单行 fit1 结果）。
    let out_df = outputs.get(&0u8).cloned().expect("无输出端口 0 数据");
    let batches = out_df.collect().await.expect("collect 输出");
    let mut total_rows = 0usize;
    for b in &batches {
        total_rows += b.num_rows();
        println!("输出 batch:\n{b:?}");
    }
    assert_eq!(total_rows, 1, "fit1 结果应为单行，实际 {total_rows}");
}
