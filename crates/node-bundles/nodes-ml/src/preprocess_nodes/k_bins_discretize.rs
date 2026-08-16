use super::*;

// ═══════════════════════════════════════════════════════════════════════
// KBinsDiscretize
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KBinsDiscretizeSpec {
    pub columns: Vec<String>,
    #[serde(default = "default_n_bins")]
    pub n_bins: usize,
    #[serde(default = "default_strategy")]
    pub strategy: String, // "uniform", "quantile"
    #[serde(default)]
    pub encode: Option<String>, // "ordinal" (default) or "onehot"
}
fn default_n_bins() -> usize {
    5
}
fn default_strategy() -> String {
    "uniform".to_string()
}

pub struct KBinsDiscretizeFactory;
impl NodeFactory for KBinsDiscretizeFactory {
    fn kind(&self) -> &'static str {
        "ml_discretize"
    }
    fn desc(&self) -> &'static str {
        "Bin continuous data into intervals."
    }
    fn doc(&self) -> &'static str {
        "KBinsDiscretizer: partitions continuous values into n_bins bins using uniform-width or quantile (median) edges. Outputs ordinal bin indices."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KBinsDiscretizeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: KBinsDiscretizeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KBinsDiscretizeNode {
            columns: s.columns,
            n_bins: s.n_bins,
            strategy: s.strategy,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct KBinsDiscretizeNode {
    columns: Vec<String>,
    n_bins: usize,
    strategy: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for KBinsDiscretizeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_discretize"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data = extract_features(&batches, &self.columns)?;
        let (nrows, ncols) = data.shape();
        let mut out = Mat::zeros(nrows, ncols);
        let use_quantile = self.strategy == "quantile";
        for j in 0..ncols {
            let col: Vec<f64> = (0..nrows).map(|i| data[(i, j)]).collect();
            let edges = if use_quantile {
                let mut sorted = col.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                (1..self.n_bins)
                    .map(|b| {
                        ml::preprocess::percentile_sorted(&sorted, b as f64 / self.n_bins as f64)
                    })
                    .collect::<Vec<_>>()
            } else {
                let cmin = col.iter().cloned().fold(f64::INFINITY, f64::min);
                let cmax = col.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let width = (cmax - cmin) / self.n_bins as f64;
                (1..self.n_bins)
                    .map(|b| cmin + b as f64 * width)
                    .collect::<Vec<_>>()
            };
            for i in 0..nrows {
                let mut bin = 0u32;
                for &edge in &edges {
                    if col[i] > edge {
                        bin += 1;
                    } else {
                        break;
                    }
                }
                out[(i, j)] = bin as f64;
            }
        }
        let batch = replace_columns(&batches, &self.columns, &out, &[])?;
        emit_batch(ctx, batch)
    }
}
