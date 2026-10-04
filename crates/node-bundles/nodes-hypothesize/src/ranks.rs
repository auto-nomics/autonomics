//! Nonparametric rank-test nodes: `hypothesize.wilcoxon`, `hypothesize.kruskal_wallis`,
//! `hypothesize.friedman_test`.

use super::common::{
    HypoNodeError, collect_input, emit_test_row, extract_f64_column, extract_groups,
};
use async_trait::async_trait;
use dag_core::dag::DagError;
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use hypothesize as h;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

fn default_alt() -> String {
    "two.sided".to_string()
}
fn default_true() -> bool {
    true
}

// ════ wilcoxon ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WilcoxonNodeSpec {
    pub x_column: String,
    pub y_column: Option<String>,
    pub group_column: Option<String>,
    #[serde(default)]
    pub mu: f64,
    #[serde(default)]
    pub paired: bool,
    #[serde(default = "default_true")]
    pub correct: bool,
    #[serde(default = "default_alt")]
    pub alternative: String,
}

pub struct WilcoxonNodeFactory;
impl NodeFactory for WilcoxonNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.wilcoxon"
    }
    fn desc(&self) -> &'static str {
        "Wilcoxon signed-rank or Mann-Whitney test."
    }
    fn doc(&self) -> &'static str {
        "Nonparametric rank-based location test."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(WilcoxonNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: WilcoxonNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(WilcoxonNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}

#[derive(Clone)]
struct WilcoxonNode {
    meta: NodePorts,
    spec: WilcoxonNodeSpec,
}

#[async_trait]
impl DagNode for WilcoxonNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.wilcoxon"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let alt = h::Alternative::parse_r(&self.spec.alternative)
            .map_err(|e| HypoNodeError::Spec(e.to_string()))?;
        let x = extract_f64_column(&batches, &self.spec.x_column)?;
        let result = if let Some(gc) = &self.spec.group_column {
            let groups = extract_groups(&batches, &self.spec.x_column, gc)?;
            if groups.len() < 2 {
                return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into());
            }
            h::mann_whitney(&groups[0].1, &groups[1].1, alt, self.spec.correct)
        } else if let Some(yc) = &self.spec.y_column {
            let y = extract_f64_column(&batches, yc)?;
            if self.spec.paired {
                // Paired Wilcoxon: test the within-pair differences x − y
                // (R: wilcox.test(x, y, paired = TRUE)). Running the one-sample
                // signed-rank on `x` alone ignores `y` entirely.
                if x.len() != y.len() {
                    return Err(HypoNodeError::Spec(format!(
                        "paired wilcoxon requires equal-length x and y columns ({} vs {})",
                        x.len(),
                        y.len()
                    ))
                    .into());
                }
                let d: Vec<f64> = x.iter().zip(&y).map(|(a, b)| a - b).collect();
                h::wilcoxon_signed_rank(
                    &d,
                    self.spec.mu,
                    alt,
                    h::ZeroMethod::Wilcox,
                    self.spec.correct,
                )
            } else {
                h::mann_whitney(&x, &y, alt, self.spec.correct)
            }
        } else {
            h::wilcoxon_signed_rank(
                &x,
                self.spec.mu,
                alt,
                h::ZeroMethod::Wilcox,
                self.spec.correct,
            )
        }
        .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ kruskal_wallis ════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KruskalWallisNodeSpec {
    pub value_column: String,
    pub group_column: String,
}

pub struct KruskalWallisNodeFactory;
impl NodeFactory for KruskalWallisNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.kruskal_wallis"
    }
    fn desc(&self) -> &'static str {
        "Kruskal-Wallis rank-sum test for k groups."
    }
    fn doc(&self) -> &'static str {
        "Nonparametric one-way ANOVA via ranks."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KruskalWallisNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: KruskalWallisNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KruskalWallisNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}

#[derive(Clone)]
struct KruskalWallisNode {
    meta: NodePorts,
    spec: KruskalWallisNodeSpec,
}

#[async_trait]
impl DagNode for KruskalWallisNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.kruskal_wallis"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let groups = extract_groups(&batches, &self.spec.value_column, &self.spec.group_column)?;
        if groups.len() < 2 {
            return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into());
        }
        let refs: Vec<&[f64]> = groups.iter().map(|(_, v)| v.as_slice()).collect();
        let result = h::kruskal_wallis(&refs).map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ friedman_test ═════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FriedmanNodeSpec {
    pub value_column: String,
    pub group_column: String,
    pub block_column: String,
}

pub struct FriedmanNodeFactory;
impl NodeFactory for FriedmanNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.friedman_test"
    }
    fn desc(&self) -> &'static str {
        "Friedman rank-sum test for randomized blocks."
    }
    fn doc(&self) -> &'static str {
        "Nonparametric test for differences across treatments in a randomized block design."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FriedmanNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: FriedmanNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(FriedmanNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}

#[derive(Clone)]
struct FriedmanNode {
    meta: NodePorts,
    spec: FriedmanNodeSpec,
}

#[async_trait]
impl DagNode for FriedmanNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.friedman_test"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        // Build blocks × treatments from value/group/block columns.
        let mut blocks: std::collections::BTreeMap<String, Vec<(String, f64)>> =
            std::collections::BTreeMap::new();
        for batch in &batches {
            use arrow_array::{Array, Float64Array};
            let vals = batch
                .column_by_name(&self.spec.value_column)
                .ok_or_else(|| HypoNodeError::Column(self.spec.value_column.clone()))?;
            let grps = batch
                .column_by_name(&self.spec.group_column)
                .ok_or_else(|| HypoNodeError::Column(self.spec.group_column.clone()))?;
            let blks = batch
                .column_by_name(&self.spec.block_column)
                .ok_or_else(|| HypoNodeError::Column(self.spec.block_column.clone()))?;
            let va = vals
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| HypoNodeError::Column("value not f64".into()))?;
            let g_strs = dag_core::node::string_opt_values(grps.as_ref()).unwrap_or_default();
            let b_strs = dag_core::node::string_opt_values(blks.as_ref()).unwrap_or_default();
            for i in 0..va.len() {
                if va.is_null(i) {
                    continue;
                }
                if let (Some(g), Some(b)) = (
                    g_strs.get(i).and_then(|x| x.as_ref()),
                    b_strs.get(i).and_then(|x| x.as_ref()),
                ) {
                    blocks
                        .entry(b.clone())
                        .or_default()
                        .push((g.clone(), va.value(i)));
                }
            }
        }
        // Pivot: find unique treatment names and build per-block rows.
        let mut treatments: Vec<String> = Vec::new();
        for entries in blocks.values() {
            for (g, _) in entries {
                if !treatments.contains(g) {
                    treatments.push(g.clone());
                }
            }
        }
        let mut data: Vec<Vec<f64>> = Vec::new(); // rows = blocks, cols = treatments
        for entries in blocks.values() {
            let row: Vec<f64> = treatments
                .iter()
                .map(|t| {
                    entries
                        .iter()
                        .find(|(g, _)| g == t)
                        .map(|(_, v)| *v)
                        .unwrap_or(f64::NAN)
                })
                .collect();
            data.push(row);
        }
        let refs: Vec<&[f64]> = data.iter().map(|r| r.as_slice()).collect();
        let result = h::friedman_test(&refs).map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

#[cfg(test)]
mod wilcoxon_paired_tests {
    use super::*;
    use std::sync::Arc;

    fn paired_batch() -> datafusion::dataframe::DataFrame {
        // x (post) = y (pre) + 0.6 with large symmetric noise: x alone is
        // sign-symmetric (one-sample signed-rank on x is null), while every
        // within-pair difference is +0.6.
        let pre = vec![5.1, -3.2, 4.8, -4.9, 3.3, -2.8, 4.1, -4.4, 2.6, -3.6];
        let post: Vec<f64> = pre.iter().map(|v| v + 0.6).collect();
        let schema = Arc::new(arrow_schema::Schema::new(vec![
            arrow_schema::Field::new("pre", arrow_schema::DataType::Float64, false),
            arrow_schema::Field::new("post", arrow_schema::DataType::Float64, false),
        ]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(arrow_array::Float64Array::from(pre)),
                Arc::new(arrow_array::Float64Array::from(post)),
            ],
        )
        .unwrap();
        datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap()
    }

    async fn run_node(spec: serde_json::Value) -> Result<f64, String> {
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let mut node = WilcoxonNodeFactory.build(spec, ctx.clone()).map_err(|e| e.to_string())?;
        let outputs = node
            .execute(
                &ctx,
                &[NodeInput::new_dataframe(0, paired_batch())],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .map_err(|e| e.to_string())?;
        let batches = outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .map_err(|e| e.to_string())?;
        let p = batches[0]
            .column(1)
            .as_any()
            .downcast_ref::<arrow_array::Float64Array>()
            .unwrap()
            .value(0);
        Ok(p)
    }

    #[tokio::test]
    async fn paired_mode_tests_within_pair_differences() {
        // Regression: paired mode used to run the one-sample signed-rank on x
        // alone (y was never read), so a constant +0.6 shift inside noisy
        // pairs came back non-significant.
        let p = run_node(serde_json::json!({
            "x_column": "post",
            "y_column": "pre",
            "paired": true,
            "alternative": "greater"
        }))
        .await
        .unwrap();
        assert!(p < 0.01, "paired wilcoxon should detect the +0.6 shift, p = {p}");
    }

    #[tokio::test]
    async fn paired_mode_rejects_length_mismatch() {
        // extract_f64_column skips nulls, so a y column with missing values
        // yields fewer entries than x → paired mode must error rather than
        // silently zip-truncate (which would misalign pairs).
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let y_with_gaps: Vec<Option<f64>> =
            vec![Some(1.0), None, Some(3.0), None, Some(5.0)];
        let schema = Arc::new(arrow_schema::Schema::new(vec![
            arrow_schema::Field::new("x", arrow_schema::DataType::Float64, false),
            arrow_schema::Field::new("y", arrow_schema::DataType::Float64, true),
        ]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(arrow_array::Float64Array::from(vec![1.1, 2.2, 3.3, 4.4, 5.5])),
                Arc::new(arrow_array::Float64Array::from(y_with_gaps)),
            ],
        )
        .unwrap();
        let df = datafusion::prelude::SessionContext::new()
            .read_batch(batch)
            .unwrap();
        let mut node = WilcoxonNodeFactory
            .build(
                serde_json::json!({"x_column": "x", "y_column": "y", "paired": true}),
                ctx.clone(),
            )
            .unwrap();
        let err = node
            .execute(
                &ctx,
                &[NodeInput::new_dataframe(0, df)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .err()
            .expect("length mismatch must error");
        assert!(
            err.to_string().contains("equal-length"),
            "error should name the mismatch: {err}"
        );
    }
}
