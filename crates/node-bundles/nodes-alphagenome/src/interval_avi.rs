//! `source_alphagenome_interval_avi` — top AVI-scored SNVs in a genomic
//! window, from the AlphaGenome Atlas.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use alphagenome::IntervalQuery;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::shared::{build_filter, make_client, single_output_port, ScoreRow};

const KIND: &str = "source_alphagenome_interval_avi";

fn default_top_k() -> usize {
    25
}

fn default_page_size() -> i32 {
    256
}

/// Spec for [`AlphagenomeIntervalAviNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct AlphagenomeIntervalAviSpec {
    /// 1-based closed GRCh38 window, e.g. `chr11:5225727-5226575`. Keep to
    /// ~1 kb; each base yields up to 3 scored SNVs.
    pub interval: String,
    /// Number of top variants (by AVI raw score, descending) to emit.
    /// Default `25`.
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Atlas page size for pagination. Default `256`.
    #[serde(default = "default_page_size")]
    pub page_size: i32,
}

/// Source node: highest-impact SNVs in a window by AVI score.
#[derive(Clone)]
pub struct AlphagenomeIntervalAviNode {
    meta: NodePorts,
    spec: AlphagenomeIntervalAviSpec,
}

pub struct AlphagenomeIntervalAviNodeFactory;

impl NodeFactory for AlphagenomeIntervalAviNodeFactory {
    fn kind(&self) -> &'static str {
        KIND
    }
    fn desc(&self) -> &'static str {
        "Scans a genomic window against the AlphaGenome Atlas and returns the \
        top SNVs by AVI score."
    }
    fn doc(&self) -> &'static str {
        "A source node that pulls the precomputed AVI score for every scored \
        GRCh38 SNV in a 1-based closed window (`interval`, e.g. \
        `chr11:5225727-5226575`) and emits the `top_k` variants with the \
        highest AVI raw scores, alongside their calibrated CDF quantile, tail \
        quantile and Phred (`-10 log10(1 - cdf)`).\n\n\
        Output columns: `chromosome, position, ref, alt, scorer, is_signed, \
        value_index, raw_score, quantile_score, tail_quantile, phred, track, \
        biosample, ontology_curie, gene_id, gene_name, error`.\n\n\
        Rank, do not threshold: pick cut-offs per region rather than using a \
        genome-wide AVI threshold. Requires `ALPHAGENOME_API_KEY`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AlphagenomeIntervalAviSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: AlphagenomeIntervalAviSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AlphagenomeIntervalAviNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
}

#[async_trait]
impl DagNode for AlphagenomeIntervalAviNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let err = |msg: String| DagError::NodeError {
            node_type: KIND.into(),
            msg,
        };
        let interval: IntervalQuery = self
            .spec
            .interval
            .parse()
            .map_err(|e| err(format!("invalid interval: {e}")))?;
        let mut client = make_client().await?;
        let filter = build_filter(&["AVI_SCORE".to_owned()], &[]);

        let all = client
            .atlas_interval_scores(&interval, Some(&filter), self.spec.page_size)
            .await
            .map_err(|e| DagError::Schedule(format!("AlphaGenome Atlas interval query failed: {e}")))?;

        let mut rows: Vec<ScoreRow> = Vec::new();
        for scores in &all {
            rows.extend(crate::shared::flatten_scores(scores));
        }
        if rows.is_empty() {
            return Err(DagError::Schedule(format!(
                "AlphaGenome Atlas returned no scored SNVs in {}",
                self.spec.interval
            )));
        }

        // Rank by AVI raw score, descending; keep the top_k.
        rows.sort_by(|a, b| {
            b.raw_score
                .partial_cmp(&a.raw_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows.truncate(self.spec.top_k.max(1));

        // Derive tail quantile (1 - cdf) and Phred from the calibrated CDF.
        let tails: Vec<Option<f64>> = rows
            .iter()
            .map(|r| match r.quantile_score {
                Some(cdf) if (0.0..1.0).contains(&cdf) => Some(1.0 - cdf),
                _ => None,
            })
            .collect();
        let phreds: Vec<Option<f64>> = tails
            .iter()
            .map(|t| t.map(|tail| -10.0 * tail.log10()))
            .collect();

        let batch = crate::shared::build_avi_batch(&rows, &tails, &phreds)?;
        let session = node_ctx.session();
        let df = session
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read AlphaGenome batch: {e}")))?;
        tracing::info!(
            "AlphaGenome interval {}: {} variants scored, top {} kept",
            self.spec.interval,
            all.len(),
            rows.len()
        );
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
