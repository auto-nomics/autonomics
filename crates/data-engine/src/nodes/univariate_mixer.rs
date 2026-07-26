//! Univariate MiXeR (`fit1`) transform node.
//!
//! 接收上游 GWAS 汇总统计 `DataFrame`（含 Z-score、样本量、rsid），从
//! Iceberg 数据湖读取 LD 矩阵（`ld_matrix.eur_chr{N}`）和 allele frequency
//! （`af.eur_af`），组装成 [`mixer::data::ChromData`]，调用
//! [`mixer::fit::fit1`] 拟合 spike-and-slab 模型，输出单行结果
//! `DataFrame`（pi, sig2_beta, sig2_zero, h2, nc, nc_p9, aic, bic, loglike）。

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory, new_isolated_ctx},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum UnivariateMixerError {
    #[error("univariate MiXeR computation not yet implemented: {0}")]
    NotImplemented(String),
    #[error("failed to read upstream sumstats: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("datalake error: {0}")]
    Datalake(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

impl From<UnivariateMixerError> for DagError {
    fn from(e: UnivariateMixerError) -> Self {
        DagError::NodeError {
            node_type: "univariate_mixer".to_string(),
            msg: e.to_string(),
        }
    }
}

impl From<datalake::error::Error> for UnivariateMixerError {
    fn from(e: datalake::error::Error) -> Self {
        UnivariateMixerError::Datalake(e.to_string())
    }
}

// =====================================================================
// Schemas
// =====================================================================

/// 上游 GWAS sumstats 固定列名：Z-score、样本量、rsid 连接键。
const INPUT_Z_COL: &str = "Z";
const INPUT_N_COL: &str = "N";
const INPUT_RSID_COL: &str = "rsid";

/// 输入端口 schema：与 ldsc_hsq 一致，便于复用同一份上游 sumstats。
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
    ]))
}

/// 输出端口 schema：单行 MiXeR fit1 结果，字段对齐
/// [`mixer::result::FitResult`]。同时作为端口声明（供 DAG 校验下游边）
/// 与 [`build_result_batch`] 的单一真相源。
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("pi", DataType::Float64, false),
        Field::new("sig2_beta", DataType::Float64, false),
        Field::new("sig2_zero", DataType::Float64, false),
        Field::new("h2", DataType::Float64, false),
        Field::new("nc", DataType::Float64, false),
        Field::new("nc_p9", DataType::Float64, false),
        Field::new("aic", DataType::Float64, false),
        Field::new("bic", DataType::Float64, false),
        Field::new("loglike", DataType::Float64, false),
    ]))
}

/// 把 [`mixer::result::FitResult`] 打包成单行 `RecordBatch`。
fn build_result_batch(r: &mixer::result::FitResult) -> Result<RecordBatch, UnivariateMixerError> {
    let schema = output_schema();
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![r.params.pi])),
            Arc::new(Float64Array::from(vec![r.params.sig2_beta])),
            Arc::new(Float64Array::from(vec![r.params.sig2_zero])),
            Arc::new(Float64Array::from(vec![r.h2])),
            Arc::new(Float64Array::from(vec![r.nc])),
            Arc::new(Float64Array::from(vec![r.nc_p9])),
            Arc::new(Float64Array::from(vec![r.aic])),
            Arc::new(Float64Array::from(vec![r.bic])),
            Arc::new(Float64Array::from(vec![r.loglike])),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Config / Spec
// =====================================================================

/// Univariate MiXeR 节点配置（DAG spec）。
///
/// 选择要参与拟合的染色体、数据湖表名，以及拟合超参数。
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UnivariateMixerNodeSpec {
    /// 参与拟合的染色体列表，如 `[21, 22]`。每个染色体对应一张
    /// `iceberg.ld_matrix.eur_chr{chrom}` 表（当前固定 EUR 人群）。
    pub chromosomes: Vec<u32>,

    /// 差分进化重复次数（原版 `--diffevo-fast-repeats`，默认 20）。
    #[serde(default = "default_diffevo_repeats")]
    pub diffevo_repeats: usize,
    /// r² 阈值：低于此值的 LD 对忽略（首版固定 sig2_zeroL=0，此字段预留）。
    #[serde(default = "default_r2_min")]
    pub r2_min: f64,
    /// 随机剪枝轮数（原版 `--randprune-n`，默认 64）。
    #[serde(default = "default_randprune_n")]
    pub randprune_n: u32,
    /// 随机剪枝 r² 阈值（原版 `--randprune-r2`，默认 0.1）。
    #[serde(default = "default_randprune_r2")]
    pub randprune_r2: f64,
    /// 随机种子（原版 `--seed`，默认 123）。
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_diffevo_repeats() -> usize {
    20
}
fn default_r2_min() -> f64 {
    0.05
}
fn default_randprune_n() -> u32 {
    64
}
fn default_randprune_r2() -> f64 {
    0.1
}
fn default_seed() -> u64 {
    123
}

// =====================================================================
// Node
// =====================================================================

const UNIVARIATE_MIXER_NODE_KIND: &str = "univariate_mixer";

/// Univariate MiXeR 拟合节点。
///
/// 输入：上游 sumstats（Z, N, rsid）。从数据湖取 LD 矩阵与 AF，组装
/// [`mixer::data::ChromData`]，调用 [`mixer::fit::fit1`]，输出单行结果。
#[derive(Clone)]
pub struct UnivariateMixerNode {
    meta: NodePorts,
    ctx: SessionContext,
    spec: UnivariateMixerNodeSpec,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

impl UnivariateMixerNode {
    pub fn new(ctx: SessionContext, spec: UnivariateMixerNodeSpec) -> Self {
        Self {
            meta: port_layout(),
            ctx,
            spec,
        }
    }
}

pub struct UnivariateMixerNodeFactory {}

impl NodeFactory for UnivariateMixerNodeFactory {
    fn kind(&self) -> &'static str {
        UNIVARIATE_MIXER_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Fits univariate MiXeR spike-and-slab (fit1) on a single GWAS trait."
    }

    fn doc(&self) -> &'static str {
        "Univariate MiXeR (fit1) transform node. Takes a single upstream GWAS \
        summary statistics DataFrame (with Z, N, rsid columns), queries the \
        Iceberg data lake for the LD matrix (`ld_matrix.eur_chr{N}`) and \
        allele frequency (`af.eur_af`), assembles a `ChromData` and fits \
        `mixer::fit::fit1`. Outputs a single-row result DataFrame with the \
        fitted parameters (pi, sig2_beta, sig2_zero) and derived quantities \
        (h2, nc, nc_p9, aic, bic, loglike). One typed input port; one typed \
        output port."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(UnivariateMixerNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: UnivariateMixerNodeSpec = serde_json::from_value(spec)?;
        let ctx = new_isolated_ctx(node_ctx.runtime_env, node_ctx.iceberg_catalog);
        let node = UnivariateMixerNode::new(ctx, config);
        Ok(Box::new(node))
    }
}

#[async_trait]
impl DagNode for UnivariateMixerNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        UNIVARIATE_MIXER_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&mut self, inputs: &[NodeInput]) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(UnivariateMixerError::InvalidInput(
            "no input DataFrame".into(),
        ))?;

        // 1. Build a fresh, isolated context per execution — no shared CatalogList,
        let ctx = &self.ctx;

        // 2. 把上游 sumstats 注册为临时表。
        ctx.register_table("sumstats", input.data.clone().into_view())?;

        // 3. 逐染色体读 AF + sumstats（join 出每个 tag 的 z/n/h），再读 LD。
        //    所有染色体合并进一个全局 index 空间——因为 LD 不跨染色体，
        //    合并后的 LdBlock 自然呈块对角，cost 与逐染色体求和等价。
        //    universe = sumstats ∩ AF（同时有 z 和 h 的 SNP）。
        //
        //    内存策略：LD **不再**累积进全局 `ld_triples`（那会让全基因组三元组
        //    常驻至循环结束）。改为对每条染色体就地把 ld_batches 流式转成本地
        //    CSR（行号本地化、列号保持全局），临时量随迭代释放；循环末尾用
        //    [`mixer::ld_matrix::LdBlock::merge_blocks`] 拼成全局块对角 CSR。
        let mut rsid_to_idx: HashMap<String, u32> = HashMap::new();
        let mut z_vec: Vec<f64> = Vec::new();
        let mut n_vec: Vec<f64> = Vec::new();
        let mut h_vec: Vec<f64> = Vec::new();
        let mut chrom_blocks: Vec<(usize, mixer::ld_matrix::LdBlock)> = Vec::new();

        for chrom in &self.spec.chromosomes {
            // 本染色体在全局 index 空间的起点（读 universe 前确定）。
            let base = rsid_to_idx.len() as u32;

            // (a) AF ∩ sumstats → 每个 tag 的 alt_freq / Z / N
            let universe_sql = format!(
                r#"SELECT a.id AS rsid, a.alt_freq AS af, s."{z}" AS zc, s."{n}" AS nc
                   FROM iceberg.af.eur_af AS a
                   INNER JOIN sumstats AS s ON a.id = s."{rsid}"
                   WHERE a.chrom = {chrom}"#,
                z = INPUT_Z_COL,
                n = INPUT_N_COL,
                rsid = INPUT_RSID_COL,
                chrom = chrom,
            );
            let universe_batches = ctx.sql(&universe_sql).await?.collect().await?;
            for batch in &universe_batches {
                let rsids = col_as_string(batch, "rsid")?;
                let afs = col_as_f64(batch, "af")?;
                let zs = col_as_f64(batch, "zc")?;
                let ns = col_as_f64(batch, "nc")?;
                for row in 0..batch.num_rows() {
                    // 重复 rsid 跳过（保持首次出现的 index）
                    let rsid = rsids.value(row);
                    if rsid_to_idx.contains_key(rsid) {
                        continue;
                    }
                    let idx = rsid_to_idx.len() as u32;
                    rsid_to_idx.insert(rsid.to_string(), idx);
                    let f = afs.value(row);
                    z_vec.push(zs.value(row));
                    n_vec.push(ns.value(row));
                    h_vec.push(2.0 * f * (1.0 - f)); // 杂合度
                }
            }

            // 本染色体 SNP 数（universe 读完即确定，行区间 [base, base+n_k) 封口）。
            let n_k = rsid_to_idx.len() - base as usize;
            if n_k == 0 {
                continue;
            }

            // (b) LD 矩阵（COO）。collect 一次物化本染色体的全部 batch，随后在
            //     内存里做 count→fill 两遍扫描——无重复查 Iceberg、无全局 triples
            //     缓冲。两端点必须在 universe 内，且 r² ≥ r2_min。
            let ld_sql = format!(
                "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{chrom}",
                chrom = chrom,
            );
            let ld_batches = ctx.sql(&ld_sql).await?.collect().await?;

            // Pass 1（计数）：每行邻居数 → row_counts。
            let mut row_counts = vec![0u32; n_k];
            for batch in &ld_batches {
                for_each_ld_entry(
                    batch,
                    base,
                    self.spec.r2_min,
                    &rsid_to_idx,
                    |local_tag, _, _| {
                        row_counts[local_tag as usize] += 1;
                    },
                )?;
            }
            // 前缀和 → row_ptr（本地行号空间）。
            let mut row_ptr = vec![0u32; n_k + 1];
            for i in 0..n_k {
                row_ptr[i + 1] = row_ptr[i] + row_counts[i];
            }
            // Pass 2（填值）：按 row_ptr 游标散布 (global_snp, r2)。col_idx 直接存
            //               全局 snp index，merge 时无需再偏移。
            let nnz_k = row_ptr[n_k] as usize;
            let mut column_index = vec![0u32; nnz_k];
            let mut r2_store = vec![0.0f64; nnz_k];
            let mut cursor = row_ptr.clone();
            for batch in &ld_batches {
                for_each_ld_entry(
                    batch,
                    base,
                    self.spec.r2_min,
                    &rsid_to_idx,
                    |local_tag, global_snp, r2| {
                        let p = cursor[local_tag as usize] as usize;
                        column_index[p] = global_snp;
                        r2_store[p] = r2;
                        cursor[local_tag as usize] += 1;
                    },
                )?;
            }

            chrom_blocks.push((
                base as usize,
                mixer::ld_matrix::LdBlock {
                    n_tag: n_k,
                    row_ptr,
                    column_index,
                    r2: r2_store,
                },
            ));
            // ld_batches、row_counts、cursor 在此随迭代结束释放。
        }

        // 4. 组装 ChromData：LD 为各染色体块拼成的全局块对角 CSR，权重全 1。
        let n_snp = z_vec.len();
        if n_snp == 0 {
            return Err(UnivariateMixerError::InvalidInput(format!(
                "no SNPs overlap between sumstats and af.eur_af for chromosomes {:?}",
                self.spec.chromosomes
            ))
            .into());
        }
        let ld = mixer::ld_matrix::LdBlock::merge_blocks(&chrom_blocks, n_snp);
        // merge_blocks 已把数据复制进全局 ld，各染色体块与 rsid 映射至此冗余，
        // fit1 前释放，避免拟合期间多占一份内存。
        drop(chrom_blocks);
        drop(rsid_to_idx);
        let mut data = mixer::data::ChromData {
            z: z_vec,
            n: n_vec,
            h: h_vec,
            weights: vec![1.0; n_snp],
            ld,
            tags: (0..n_snp as u32).collect(),
        };

        // 4b. 随机剪枝权重（生产级精度，与原版 `set_weights_randprune` bit-exact）。
        //     tag 集 = 全部 SNP（节点不做 extract）；权重覆盖到 data.weights。
        let tags: Vec<u32> = (0..n_snp as u32).collect();
        let rp_cfg = mixer::weights::RandpruneConfig {
            n: self.spec.randprune_n,
            r2_threshold: self.spec.randprune_r2,
            use_w_ld: false,
            seed: self.spec.seed,
        };
        data.weights = mixer::weights::randprune_weights(&data.ld, n_snp, &tags, None, &rp_cfg);

        // 4c. 压缩成充分统计量（m1/m2）：单趟扫全局块对角 CSR，把每个 SNP 的
        //     LD 邻居求和折进两个标量。此后 cost 求值 O(1)/tag，不再需要 LD。
        //     randprune 已完成（它需要 CSR + 全局 tag 空间，无法流式），故此处
        //     可把整份 ChromData（含 ld/n/h，O(nnz)）释放，只留 O(n_snp) 的
        //     充分统计量进入漫长的拟合阶段（DE×repeats → NM 的数万次评估）。
        let suff = mixer::data::UnivariateSufficient::from_chrom_data(&data);
        drop(data);

        // 5. 跑 fit1（DE×repeats → Nelder-Mead 精修），只读充分统计量。
        let cfg = mixer::fit::FitConfig {
            diffevo_repeats: self.spec.diffevo_repeats,
            ..Default::default()
        };
        let result = mixer::fit::fit1(&suff, &cfg);

        // 6. 打包单行结果 RecordBatch 并返回。
        let batch = build_result_batch(&result)?;
        let df = ctx.read_batch(batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// Arrow 列提取 helpers
// =====================================================================

/// 按列名取 `StringArray`（rsid 等 Utf8 列）。
fn col_as_string<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a StringArray, UnivariateMixerError> {
    batch
        .column_by_name(name)
        .ok_or_else(|| UnivariateMixerError::InvalidInput(format!("column '{name}' not found")))?
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| UnivariateMixerError::InvalidInput(format!("column '{name}' is not Utf8")))
}

/// 按列名取 `Float64Array`（Z/N/alt_freq/unphased_r2 等）。
///
/// 要求上游 schema 为 Float64（AF/LD 表与输入端口 schema 均为 Float64）。
fn col_as_f64<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a Float64Array, UnivariateMixerError> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| UnivariateMixerError::InvalidInput(format!("column '{name}' not found")))?;
    col.as_any().downcast_ref::<Float64Array>().ok_or_else(|| {
        UnivariateMixerError::InvalidInput(format!(
            "column '{name}' is not Float64 (got {})",
            col.data_type()
        ))
    })
}

/// 对一条 LD batch 施加与原版一致过滤（`r2 ≥ r2_min` 且两端点都在 universe
/// 内），对每个存活项调用 `emit(local_tag, global_snp, r2)`。
///
/// 计数遍与填值遍共用这一份代码路径，保证两遍看到完全相同的条目集合——
/// `local_tag = global_tag - base`（本染色体行区间内的本地行号），
/// `global_snp` 保留全局 index（直接写入 CSR 的 `column_index`）。
fn for_each_ld_entry(
    batch: &RecordBatch,
    base: u32,
    r2_min: f64,
    rsid_to_idx: &HashMap<String, u32>,
    mut emit: impl FnMut(u32, u32, f64),
) -> Result<(), UnivariateMixerError> {
    let a_ids = col_as_string(batch, "id_a")?;
    let b_ids = col_as_string(batch, "id_b")?;
    let r2s = col_as_f64(batch, "unphased_r2")?;
    for row in 0..batch.num_rows() {
        let r2 = r2s.value(row);
        if r2 < r2_min {
            continue;
        }
        let a = a_ids.value(row);
        let b = b_ids.value(row);
        if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
            // ta 应属于本染色体区间 [base, base+n_k)；checked_sub 防御下溢。
            if let Some(local_tag) = ta.checked_sub(base) {
                emit(local_tag, tb, r2);
            }
        }
    }
    Ok(())
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_type_is_stable() {
        assert_eq!(UNIVARIATE_MIXER_NODE_KIND, "univariate_mixer");
    }

    #[tokio::test]
    async fn construct_node_with_spec() {
        // 仅验证节点能按 spec 构造、端口 schema 正确。
        let spec = UnivariateMixerNodeSpec {
            chromosomes: vec![21, 22],
            diffevo_repeats: 5,
            r2_min: 0.05,
            randprune_n: 64,
            randprune_r2: 0.1,
            seed: 123,
        };
        let node = UnivariateMixerNode::new(SessionContext::new(), spec);
        assert_eq!(node.kind(), "univariate_mixer");
        // 一个输入端口（sumstats）、一个输出端口（fit1 结果）
        assert_eq!(node.ports().input_ports().len(), 1);
        assert_eq!(node.ports().output_ports().len(), 1);
    }
    //
    // #[tokio::test]
    // async fn load_iceberg_ld_matrix_panel() {
    //     let dk = Datalake::default();
    //     let ctx = dk.get_ctx().await.unwrap();
    //     let ld_df_chr22 = ctx.sql("SELECT * FROM iceberg.af.eur_af").await.unwrap();
    //     ld_df_chr22
    //         .limit(0, Some(10))
    //         .unwrap()
    //         .show()
    //         .await
    //         .unwrap();
    //     panic!()
    // }
}
