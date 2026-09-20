//! `paired_bs_delta` — the primary-endpoint node: paired IPCW Brier delta
//! with bootstrap inference for two competing-risk prediction columns on the
//! same subjects.
//!
//! This is the mechanism behind the SAP's ΔBS = BS(M0) − BS(M3) at the
//! horizon: `prob_a` carries the baseline model's predicted CIF (e.g. the
//! clinical-only model M0), `prob_b` the extended model's (e.g. M3 with the
//! proteomic increment added), and the bootstrap resamples subjects to put a
//! percentile and a basic CI plus a two-sided p-value on their paired
//! difference. Positive deltas mean `prob_b` (the extended model)
//! discriminates better at the horizon.
//!
//! One extra `ipcw_brier` pass over `prob_a` supplies the censoring
//! diagnostics (`g_at_horizon`, `min_g_upto_horizon`, `max_weight`,
//! `n_extreme_weights`, event accounting) so a frozen report can flag heavy
//! censoring without re-running anything.

use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use crrkit::bootstrap::{BootstrapOptions, paired_brier_delta_bootstrap};
use crrkit::brier::{BrierOptions, ipcw_brier};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::cv::recode_status;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

/// Node kind string.
pub const PAIRED_BS_DELTA_NODE_KIND: &str = "paired_bs_delta";

fn d_failcode() -> f64 {
    1.0
}
fn d_cencode() -> f64 {
    0.0
}
fn d_extreme() -> f64 {
    10.0
}
fn d_floor() -> f64 {
    1e-5
}
fn d_nboot() -> usize {
    2000
}
fn d_alpha() -> f64 {
    0.05
}

/// Configuration for the `paired_bs_delta` node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PairedBsDeltaSpec {
    /// Failure / censoring time column.
    pub time_column: String,
    /// Failure-type code column (`failcode` = event, `cencode` = censored,
    /// anything else = competing event).
    pub status_column: String,
    /// Predicted CIF at `horizon` from model A — the baseline (e.g. the
    /// clinical-only model M0).
    pub prob_a_column: String,
    /// Predicted CIF at `horizon` from model B — the extended model (e.g.
    /// M3); the delta is `BS(A) − BS(B)`, positive meaning B better.
    pub prob_b_column: String,
    /// Evaluation horizon `t*` (days) for both Brier scores.
    pub horizon: f64,
    /// Code of `status_column` denoting the event of interest.
    #[serde(default = "d_failcode")]
    pub failcode: f64,
    /// Code of `status_column` denoting censoring.
    #[serde(default = "d_cencode")]
    pub cencode: f64,
    /// IPCW weights above this multiple of unity are counted in
    /// `n_extreme_weights` (a censoring-heaviness flag).
    #[serde(default = "d_extreme")]
    pub extreme_weight_threshold: f64,
    /// Floor on the reverse-KM censoring survival used for the weights.
    #[serde(default = "d_floor")]
    pub g_floor: f64,
    /// Bootstrap resamples for the CI and p-value.
    #[serde(default = "d_nboot")]
    pub n_boot: usize,
    /// Seed for the deterministic bootstrap resampling.
    #[serde(default)]
    pub seed: u64,
    /// Two-sided alpha of the confidence intervals.
    #[serde(default = "d_alpha")]
    pub alpha: f64,
}

impl PairedBsDeltaSpec {
    fn validate(&self) -> Result<(), String> {
        for (label, col) in [
            ("time_column", &self.time_column),
            ("status_column", &self.status_column),
            ("prob_a_column", &self.prob_a_column),
            ("prob_b_column", &self.prob_b_column),
        ] {
            if col.trim().is_empty() {
                return Err(format!("{label} must be non-empty"));
            }
        }
        if self.prob_a_column == self.prob_b_column {
            return Err(
                "prob_a_column and prob_b_column must differ — a paired delta against \
                 itself is always zero"
                    .into(),
            );
        }
        if !self.horizon.is_finite() || self.horizon <= 0.0 {
            return Err("horizon must be finite and positive".into());
        }
        if !self.extreme_weight_threshold.is_finite() || self.extreme_weight_threshold <= 0.0 {
            return Err("extreme_weight_threshold must be finite and positive".into());
        }
        if !self.g_floor.is_finite() || self.g_floor <= 0.0 || self.g_floor >= 1.0 {
            return Err("g_floor must lie in (0, 1)".into());
        }
        if self.n_boot == 0 {
            return Err("n_boot must be >= 1".into());
        }
        if !self.alpha.is_finite() || self.alpha <= 0.0 || self.alpha >= 1.0 {
            return Err("alpha must lie in (0, 1)".into());
        }
        Ok(())
    }
}

pub struct PairedBsDeltaNodeFactory {}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None) // 0: per-subject table with time/status/prob_a/prob_b
        .add_output_port(None) // 0: single-row summary
}

impl NodeFactory for PairedBsDeltaNodeFactory {
    fn kind(&self) -> &'static str {
        PAIRED_BS_DELTA_NODE_KIND
    }
    fn desc(&self) -> &'static str {
        "Paired IPCW Brier-score delta with bootstrap CI for two competing-risk models."
    }
    fn doc(&self) -> &'static str {
        "Computes delta = BS(A) - BS(B), the difference of the IPCW Brier \
        scores (Schoop et al. 2011) of two sets of predicted cumulative-\
        incidence probabilities at the horizon on the same subjects — the \
        SAP's primary endpoint reads BS(A) from the clinical-only model M0 \
        and BS(B) from the extended model M3, so a positive delta with a CI \
        excluding zero favors the extended model. Percentile and basic \
        bootstrap CIs and a two-sided p-value come from subject-level \
        resampling (n_boot, deterministic under seed). Rows with a missing \
        value in any used column are dropped listwise and counted in \
        n_dropped_missing; the censoring diagnostics (reverse-KM G at the \
        horizon, max IPCW weight, extreme-weight count) flag heavy censoring \
        where the score rests on few subjects."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PairedBsDeltaSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: PairedBsDeltaSpec = serde_json::from_value(spec)?;
        if let Err(reason) = s.validate() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: PAIRED_BS_DELTA_NODE_KIND.to_string(),
                reason,
                schema_pretty: serde_json::to_string_pretty(&schema_for!(PairedBsDeltaSpec))
                    .unwrap_or_default(),
            });
        }
        Ok(Box::new(PairedBsDeltaNode {
            meta: port_layout(),
            spec: s,
        }))
    }
}

#[derive(Clone)]
pub struct PairedBsDeltaNode {
    meta: NodePorts,
    spec: PairedBsDeltaSpec,
}

fn err(msg: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: PAIRED_BS_DELTA_NODE_KIND.into(),
        msg: msg.into(),
    }
}

#[async_trait]
impl DagNode for PairedBsDeltaNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        PAIRED_BS_DELTA_NODE_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| err("no input connected"))?;
        let batches = input
            .dataframe()?
            .clone()
            .collect()
            .await
            .map_err(|e| err(format!("collect: {e}")))?;
        let s = &self.spec;

        let time = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.time_column)
            .map_err(|e| err(e.to_string()))?;
        let fstatus = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.status_column)
            .map_err(|e| err(e.to_string()))?;
        let prob_a = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.prob_a_column)
            .map_err(|e| err(e.to_string()))?;
        let prob_b = dag_core::arrow_util::extract_numeric_lenient(&batches, &s.prob_b_column)
            .map_err(|e| err(e.to_string()))?;

        let n = time.len();
        let mut keep = vec![true; n];
        let mut n_dropped = 0usize;
        for i in 0..n {
            let bad = !time[i].is_finite()
                || !fstatus[i].is_finite()
                || !prob_a[i].is_finite()
                || !prob_b[i].is_finite();
            if bad {
                keep[i] = false;
                n_dropped += 1;
            }
        }
        let pos: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
        let t: Vec<f64> = pos.iter().map(|&i| time[i]).collect();
        let status = recode_status(
            &(pos.iter().map(|&i| fstatus[i]).collect::<Vec<_>>()),
            s.failcode,
            s.cencode,
        )
        .map_err(err)?;
        let pa: Vec<f64> = pos.iter().map(|&i| prob_a[i]).collect();
        let pb: Vec<f64> = pos.iter().map(|&i| prob_b[i]).collect();
        if t.is_empty() {
            return Err(err("no complete cases survived the listwise drop"));
        }

        let brier_opts = BrierOptions {
            extreme_weight_threshold: s.extreme_weight_threshold,
            g_floor: s.g_floor,
        };
        let boot_opts = BootstrapOptions {
            n_boot: s.n_boot,
            seed: s.seed,
            alpha: s.alpha,
        };
        let delta =
            paired_brier_delta_bootstrap(&t, &status, &pa, &pb, s.horizon, &brier_opts, &boot_opts)
                .map_err(|e| err(format!("paired bootstrap: {e}")))?;
        let diag = ipcw_brier(&t, &status, &pa, s.horizon, &brier_opts)
            .map_err(|e| err(format!("censoring diagnostics: {e}")))?;

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("horizon", DataType::Float64, false),
                Field::new("n", DataType::UInt32, false),
                Field::new("n_dropped_missing", DataType::UInt32, false),
                Field::new("n_cause1", DataType::UInt32, false),
                Field::new("n_competing", DataType::UInt32, false),
                Field::new("n_beyond_horizon", DataType::UInt32, false),
                Field::new("n_censored_before_horizon", DataType::UInt32, false),
                Field::new("brier_a", DataType::Float64, false),
                Field::new("brier_b", DataType::Float64, false),
                Field::new("delta", DataType::Float64, false),
                Field::new("ci_percentile_lower", DataType::Float64, false),
                Field::new("ci_percentile_upper", DataType::Float64, false),
                Field::new("ci_basic_lower", DataType::Float64, false),
                Field::new("ci_basic_upper", DataType::Float64, false),
                Field::new("p_value", DataType::Float64, false),
                Field::new("n_boot", DataType::UInt32, false),
                Field::new("n_usable", DataType::UInt32, false),
                Field::new("n_failed", DataType::UInt32, false),
                Field::new("g_at_horizon", DataType::Float64, false),
                Field::new("min_g_upto_horizon", DataType::Float64, false),
                Field::new("max_weight", DataType::Float64, false),
                Field::new("n_extreme_weights", DataType::UInt32, false),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![s.horizon])),
                Arc::new(UInt32Array::from(vec![t.len() as u32])),
                Arc::new(UInt32Array::from(vec![n_dropped as u32])),
                Arc::new(UInt32Array::from(vec![diag.n_cause1 as u32])),
                Arc::new(UInt32Array::from(vec![diag.n_competing as u32])),
                Arc::new(UInt32Array::from(vec![diag.n_beyond_horizon as u32])),
                Arc::new(UInt32Array::from(vec![
                    diag.n_censored_before_horizon as u32,
                ])),
                Arc::new(Float64Array::from(vec![delta.brier_a])),
                Arc::new(Float64Array::from(vec![delta.brier_b])),
                Arc::new(Float64Array::from(vec![delta.point_delta])),
                Arc::new(Float64Array::from(vec![delta.ci_percentile.0])),
                Arc::new(Float64Array::from(vec![delta.ci_percentile.1])),
                Arc::new(Float64Array::from(vec![delta.ci_basic.0])),
                Arc::new(Float64Array::from(vec![delta.ci_basic.1])),
                Arc::new(Float64Array::from(vec![delta.p_value])),
                Arc::new(UInt32Array::from(vec![s.n_boot as u32])),
                Arc::new(UInt32Array::from(vec![delta.n_usable as u32])),
                Arc::new(UInt32Array::from(vec![delta.n_failed as u32])),
                Arc::new(Float64Array::from(vec![diag.g_at_horizon])),
                Arc::new(Float64Array::from(vec![diag.min_g_upto_horizon])),
                Arc::new(Float64Array::from(vec![diag.max_weight])),
                Arc::new(UInt32Array::from(vec![diag.n_extreme_weights as u32])),
            ],
        )
        .map_err(|e| err(format!("summary batch: {e}")))?;

        let ctx = node_ctx.session();
        let mut res = PortOutputs::new();
        res.insert(
            0,
            ctx.read_batch(batch)
                .map_err(|e| err(format!("read_batch: {e}")))?,
        );
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: serde_json::Value) -> PairedBsDeltaSpec {
        serde_json::from_value(json).expect("spec parses")
    }

    fn base() -> serde_json::Value {
        serde_json::json!({
            "time_column": "t", "status_column": "s",
            "prob_a_column": "p_m0", "prob_b_column": "p_m3",
            "horizon": 1095.0
        })
    }

    #[test]
    fn defaults_and_validation() {
        let s = spec(base());
        assert_eq!(s.failcode, 1.0);
        assert_eq!(s.cencode, 0.0);
        assert_eq!(s.extreme_weight_threshold, 10.0);
        assert_eq!(s.g_floor, 1e-5);
        assert_eq!(s.n_boot, 2000);
        assert_eq!(s.seed, 0);
        assert_eq!(s.alpha, 0.05);
        assert!(s.validate().is_ok());

        let mut same = base();
        same["prob_b_column"] = "p_m0".into();
        assert!(
            spec(same).validate().is_err(),
            "identical prob columns rejected"
        );

        let mut bad_alpha = base();
        bad_alpha["alpha"] = 1.5.into();
        assert!(spec(bad_alpha).validate().is_err());

        let mut bad_horizon = base();
        bad_horizon["horizon"] = 0.0.into();
        assert!(spec(bad_horizon).validate().is_err());

        let mut bad_boot = base();
        bad_boot["n_boot"] = 0.into();
        assert!(spec(bad_boot).validate().is_err());
    }

    #[test]
    fn delta_rewards_the_better_model_and_is_seed_deterministic() {
        // Deterministic competing-risks sample: cause-1 hazard rises with x.
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                let mut x = self.0;
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                self.0 = x;
                x.wrapping_mul(0x2545F4914F6CDD1D)
            }
            fn uniform(&mut self) -> f64 {
                (self.next() >> 11) as f64 / (1u64 << 53) as f64
            }
            fn exponential(&mut self, rate: f64) -> f64 {
                -(self.uniform().max(1e-12)).ln() / rate
            }
        }
        let mut rng = Rng(41);
        let n = 400;
        let horizon = 30.0;
        let mut t = Vec::with_capacity(n);
        let mut status = Vec::with_capacity(n);
        let mut pa = Vec::with_capacity(n);
        let mut pb = Vec::with_capacity(n);
        for _ in 0..n {
            let x = rng.uniform() * 2.0 - 1.0;
            let t1 = rng.exponential(0.05 * (0.8 * x).exp());
            let t2 = rng.exponential(0.06);
            let c = rng.exponential(0.03);
            if t1 <= t2 && t1 <= c {
                t.push(t1);
                status.push(1u8);
            } else if t2 < t1 && t2 <= c {
                t.push(t2);
                status.push(2u8);
            } else {
                t.push(c);
                status.push(0u8);
            }
            // M0 ignores x; M3 tracks it (roughly monotone in the risk score).
            pa.push(0.30);
            pb.push((0.30 + 0.10 * x).clamp(0.01, 0.99));
        }

        let opts = BrierOptions::default();
        let boot = |seed: u64| {
            paired_brier_delta_bootstrap(
                &t,
                &status,
                &pa,
                &pb,
                horizon,
                &opts,
                &BootstrapOptions {
                    n_boot: 400,
                    seed,
                    alpha: 0.05,
                },
            )
            .unwrap()
        };
        let a = boot(7);
        let b = boot(7);
        assert_eq!(a.point_delta, b.point_delta, "same seed is deterministic");
        assert_eq!(a.ci_percentile, b.ci_percentile);
        assert_eq!(a.p_value, b.p_value);

        let c = boot(8);
        assert_eq!(a.point_delta, c.point_delta, "point estimate is seed-free");
        assert_ne!(a.p_value, c.p_value, "resampling differs across seeds");

        assert!(
            a.point_delta > 0.0,
            "M3 must beat M0 here: {}",
            a.point_delta
        );
        assert!((0.0..=1.0).contains(&a.brier_a));
        assert!((0.0..=1.0).contains(&a.brier_b));
        assert!((0.0..=1.0).contains(&a.p_value));
        assert!(a.ci_percentile.0 <= a.ci_percentile.1);
        assert!(a.n_usable > 0);
    }
}
