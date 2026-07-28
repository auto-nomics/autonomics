//! `UnivariateMixerNode` 端到端 + 输出校验 + 阶段耗时：跑真实 BMI/chr22 sumstats
//! 走完整个 `execute`（universe → extract → LD fold → fit1），对 fit1 结果做物理
//! 合理性断言（参数有限、h²∈[0,1]、π 不贴边界、loglike 有限），并**捕获
//! NodeReporter 事件按已知阶段标记切分四阶段耗时**。
//!
//! 这是**当前节点计算状况的实际检测**——验证并行化 + ahash + 日志发射等优化
//! 后，节点仍能跑通且输出符合物理约束；阶段耗时指出瓶颈在哪。
//! `#[ignore]`，需连真实 Iceberg 数据湖。
//!
//! 手动运行：
//! ```sh
//! cargo test -p data-engine --test mixer_node_validated_e2e -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow_array::Float64Array;
use datalake::Datalake;
use tokio::sync::mpsc;

use data_engine::dag::node_event::{NodeEvent, NodeEventKind, NodeReporter};
use data_engine::node_registry::registry::NodeCtx;
use data_engine::nodes::meta::{DagNode, NodeInput};
use data_engine::nodes::univariate_mixer::{UnivariateMixerNode, UnivariateMixerNodeSpec};

/// 输出端口 schema 字段名（顺序与 `output_schema()` 一致）。
const OUTPUT_NAMES: [&str; 9] = [
    "pi",
    "sig2_beta",
    "sig2_zero",
    "h2",
    "nc",
    "nc_p9",
    "aic",
    "bic",
    "loglike",
];

/// 阶段标记：从 NodeEvent Log 消息里识别（start → end 配对算耗时）。
/// 与 `univariate_mixer::execute` 里 `reporter.info` 的措辞严格对应。
const PHASE_MARKERS: &[(&'static str, &'static str)] = &[
    ("universe_start", "fit1: start ("),
    ("extract_start", "extract: building r²>"),
    ("extract_end", "SNPs selected as tags"),
    ("fold_start", "LD fold: mode="),
    ("fold_end", "LD fold: "), // 匹配"LD fold: N pairs folded"，不会匹配到 per-chrom 的"LD fold chr{...}:"
    ("fit1_start", "fit1: entering optimization"),
    ("fit1_end", "fit1: finished in"),
];

#[tokio::test]
#[ignore]
async fn mixer_node_validated_e2e() {
    let dk = Arc::new(Datalake::new());

    // 1. 上游 sumstats: BMI（ieu-a-2）/ chr22。
    let ctx = dk.get_ctx().await.expect("无法连 Iceberg 数据湖");
    let node_ctx = NodeCtx {
        runtime_env: ctx.runtime_env(),
        iceberg_catalog: Some(Arc::new(
            dk.get_provider().await.expect("datalake provider"),
        )),
        datalake: dk.clone(),
        opendal: None,
    };
    let sumstats = ctx
        .sql(
            r#"SELECT rsid, effect_size / std_error AS "Z", sample_size AS "N"
               FROM iceberg.gwas.bmi_ieu_a_2
               WHERE chrom = '22' AND std_error > 0 AND effect_size IS NOT NULL"#,
        )
        .await
        .expect("sumstats SQL");

    let n_in = sumstats.clone().count().await.expect("count sumstats");
    println!("[setup] 上游 sumstats 行数: {n_in}");
    assert!(n_in > 1000, "sumstats 行数过少 ({n_in})，无法做有意义测试");

    // 2. 构造节点：chr22，**面板模式**（从 Iceberg 查 tag 表 + 子图表，跳过 extract + ld_matrix 扫描）。
    let spec = UnivariateMixerNodeSpec {
        chromosomes: vec![22],
        diffevo_repeats: 2,
        r2_min: 0.05,
        seed: 123,
        extract_enabled: true,
        extract_maf: 0.05,
        extract_subset: 2_000_000,
        extract_r2: 0.8,
    };
    let mut node = UnivariateMixerNode::new(spec);

    // 3. 设自定义 reporter，把事件捕到本地（用接收时刻作为事件时间戳）。
    let (tx, mut rx) = mpsc::channel::<NodeEvent>(4096);
    let reporter = NodeReporter::new("mixer_validated_e2e", tx);
    let capture = tokio::spawn(async move {
        let t0 = Instant::now();
        let mut events: Vec<(Duration, NodeEvent)> = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push((t0.elapsed(), ev));
        }
        events
    });

    // 4. 执行（计时整体 + 收集事件）。
    let wall_start = Instant::now();
    let outputs = node
        .execute(
            &node_ctx,
            &[NodeInput {
                port: 0,
                data: sumstats,
            }],
            &reporter,
        )
        .await
        .expect("execute 失败");
    let wall = wall_start.elapsed();
    drop(reporter); // 关 channel，让 capture task 退出
    let events = capture.await.expect("capture task panic");
    println!("[execute] 整体 wall-clock: {:.2?}", wall);
    println!("[execute] 捕获到 {} 个 NodeEvent", events.len());

    // 5. 按阶段标记切分耗时。
    let mut hits: HashMap<&'static str, Duration> = HashMap::new();
    for (elapsed, ev) in &events {
        if let NodeEventKind::Log { message, .. } = &ev.kind {
            for (name, prefix) in PHASE_MARKERS {
                if hits.contains_key(name) {
                    continue; // 第一次匹配为准（per-chrom 的 fold_end 会被忽略，因为"LD fold: "匹配的非 fold_end 先来）
                }
                if message.starts_with(prefix) {
                    hits.insert(*name, *elapsed);
                }
            }
        }
    }
    let phase =
        |a: &str, b: &str| -> Option<Duration> { Some(hits.get(b)?.checked_sub(*hits.get(a)?)?) };
    let t_universe = phase("universe_start", "extract_start");
    let t_extract = phase("extract_start", "extract_end");
    let t_fold = phase("fold_start", "fold_end");
    let t_fit1 = phase("fit1_start", "fit1_end");
    println!(
        "[phase] universe     : {:>8.2?}",
        t_universe.unwrap_or_default()
    );
    println!(
        "[phase] extract      : {:>8.2?}",
        t_extract.unwrap_or_default()
    );
    println!(
        "[phase] LD fold      : {:>8.2?}",
        t_fold.unwrap_or_default()
    );
    println!(
        "[phase] fit1 (DE+NM) : {:>8.2?}",
        t_fit1.unwrap_or_default()
    );
    let sum_phases = t_universe.unwrap_or_default()
        + t_extract.unwrap_or_default()
        + t_fold.unwrap_or_default()
        + t_fit1.unwrap_or_default();
    println!(
        "[phase] 四阶段求和   : {:>8.2?}（wall={:.2?}，差值≈setup/serde 等开销）",
        sum_phases, wall
    );

    // 6. 收集并解析 fit1 单行结果。
    let out_df = outputs.get(&0u8).cloned().expect("无输出端口 0 数据");
    let batches = out_df.collect().await.expect("collect 输出");
    let mut total_rows = 0usize;
    let mut values: HashMap<String, f64> = HashMap::new();
    for b in &batches {
        total_rows += b.num_rows();
        for (i, name) in OUTPUT_NAMES.iter().enumerate() {
            if let Some(col) = b.column(i).as_any().downcast_ref::<Float64Array>() {
                if col.len() > 0 {
                    values.insert((*name).to_string(), col.value(0));
                }
            }
        }
        println!("[output] batch:\n{b:?}");
    }
    assert_eq!(total_rows, 1, "fit1 应返回单行，实际 {total_rows}");
    assert_eq!(
        values.len(),
        OUTPUT_NAMES.len(),
        "输出字段缺失：got {}，want {}",
        values.len(),
        OUTPUT_NAMES.len()
    );

    // 7. 物理合理性断言。
    for name in OUTPUT_NAMES {
        let v = values[name];
        assert!(v.is_finite(), "{name}={v} 非有限（NaN/Inf）");
    }
    let pi = values["pi"];
    assert!((0.0..=1.0).contains(&pi), "pi={pi} 超出 [0,1]");
    assert!(
        pi > 1e-4 && pi < 0.999,
        "pi={pi} 贴 0/1 边界（spike-and-slab 退化）"
    );
    let sig2_beta = values["sig2_beta"];
    let sig2_zero = values["sig2_zero"];
    assert!(sig2_beta >= 0.0, "sig2_beta={sig2_beta} < 0");
    assert!(sig2_zero >= 0.0, "sig2_zero={sig2_zero} < 0");
    let h2 = values["h2"];
    assert!((0.0..=1.0).contains(&h2), "h2={h2} 超出 [0,1]");
    assert!(h2 >= 0.0, "h2={h2} < 0");
    let nc = values["nc"];
    let nc_p9 = values["nc_p9"];
    assert!(nc >= 0.0, "nc={nc} < 0");
    assert!(nc_p9 >= 0.0, "nc_p9={nc_p9} < 0");

    println!(
        "[validate] ✓ 9 个输出字段均有限，且 h²∈[0,1]、π∈(0.001, 0.999)、方差非负、loglike 有限"
    );
    println!(
        "[result] pi={:.4} sig2_beta={:.4} sig2_zero={:.4} h2={:.4} nc={:.0} nc_p9={:.0} loglike={:.2} aic={:.2} bic={:.2}",
        pi, sig2_beta, sig2_zero, h2, nc, nc_p9, values["loglike"], values["aic"], values["bic"],
    );
}
