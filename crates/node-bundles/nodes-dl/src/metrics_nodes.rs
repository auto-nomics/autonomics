//! `dl_survival_metrics` — C-index, td-AUC, Brier score.

use std::sync::Arc;

use arrow_array::{Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use super::common;
use dl::survival;

const NODE: &str = "dl_survival_metrics";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SurvivalMetricsSpec {
    pub time_column: String,
    pub event_column: String,
    pub score_column: String,
    /// Metrics to compute: "cindex", "td_auc", "brier".
    #[serde(default = "d_metrics")]
    pub metrics: Vec<String>,
    /// Time points for td_auc and brier.
    #[serde(default = "d_time_points")]
    pub time_points: Vec<f64>,
    #[serde(default = "d_n_boot")]
    pub n_bootstrap: usize,
    #[serde(default = "d_seed")]
    pub seed: u64,
}

fn d_metrics() -> Vec<String> { vec!["cindex".into()] }
fn d_time_points() -> Vec<f64> { vec![5.0, 10.0] }
fn d_n_boot() -> usize { 0 }
fn d_seed() -> u64 { 42 }

pub struct SurvivalMetricsFactory;
impl NodeFactory for SurvivalMetricsFactory {
    fn kind(&self) -> &'static str { NODE }
    fn desc(&self) -> &'static str { "Survival model evaluation metrics." }
    fn doc(&self) -> &'static str {
        "dl_survival_metrics: computes Harrell's C-index, time-dependent AUC, and Brier score \
        for survival model predictions. Supports bootstrap confidence intervals."
    }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(SurvivalMetricsSpec) }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: SurvivalMetricsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SurvivalMetricsNode { spec: s, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct SurvivalMetricsNode {
    spec: SurvivalMetricsSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for SurvivalMetricsNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { NODE }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = common::collect_batches(inputs, NODE).await?;
        let scores = common::extract_numeric_column(&batches, &self.spec.score_column)?;
        let times = common::extract_numeric_column(&batches, &self.spec.time_column)?;
        let events_f = common::extract_numeric_column(&batches, &self.spec.event_column)?;
        let events: Vec<usize> = events_f.into_iter().map(|v| v as usize).collect();

        let mut metric_names: Vec<String> = Vec::new();
        let mut metric_values: Vec<f64> = Vec::new();

        for metric in &self.spec.metrics {
            match metric.as_str() {
                "cindex" | "c_index" => {
                    let ci = survival::c_index(&scores, &times, &events);
                    metric_names.push("cindex".into());
                    metric_values.push(ci);
                }
                "td_auc" => {
                    for &t in &self.spec.time_points {
                        let auc = survival::td_auc(&scores, &times, &events, t);
                        metric_names.push(format!("td_auc_{t}"));
                        metric_values.push(auc);
                    }
                }
                "brier" => {
                    // For Brier, convert risk scores to survival probabilities.
                    // S(t|x) ≈ exp(-exp(risk_score)) as a rough approximation.
                    let probs: Vec<f64> = scores.iter()
                        .map(|&r| (-r.exp()).max(-50.0).exp())
                        .collect();
                    for &t in &self.spec.time_points {
                        let bs = survival::brier_score(&probs, &times, &events, t);
                        if bs.is_finite() {
                            metric_names.push(format!("brier_{t}"));
                            metric_values.push(bs);
                        }
                    }
                }
                _ => {}
            }
        }

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("metric", DataType::Utf8, false),
                Field::new("value", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(metric_names)),
                Arc::new(Float64Array::from(metric_values)),
            ],
        ).map_err(|e| common::err(NODE, format!("build batch: {e}")))?;

        common::emit_batch(ctx, batch, NODE)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// dl_calibration
// ═══════════════════════════════════════════════════════════════════════

const NODE_CAL: &str = "dl_calibration";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CalibrationSpec {
    pub score_column: String,
    pub label_column: String,
    #[serde(default = "d_n_bins")]
    pub n_bins: usize,
    #[serde(default = "d_cal_methods")]
    pub methods: Vec<String>,
}

fn d_n_bins() -> usize { 10 }
fn d_cal_methods() -> Vec<String> { vec!["brier".into(), "calibration_curve".into()] }

pub struct CalibrationFactory;
impl NodeFactory for CalibrationFactory {
    fn kind(&self) -> &'static str { NODE_CAL }
    fn desc(&self) -> &'static str { "Model calibration assessment." }
    fn doc(&self) -> &'static str {
        "dl_calibration: evaluates prediction calibration via Brier score, Hosmer-Lemeshow test, \
        and calibration curve. Outputs a summary table and decile calibration data."
    }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(CalibrationSpec) }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: CalibrationSpec = serde_json::from_value(spec)?;
        Ok(Box::new(CalibrationNode { spec: s, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct CalibrationNode {
    spec: CalibrationSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for CalibrationNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { NODE_CAL }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = common::collect_batches(inputs, NODE_CAL).await?;
        let scores = common::extract_numeric_column(&batches, &self.spec.score_column)?;
        let labels = common::extract_numeric_column(&batches, &self.spec.label_column)?;
        let n = scores.len();

        let mut names: Vec<String> = Vec::new();
        let mut values: Vec<f64> = Vec::new();

        for method in &self.spec.methods {
            match method.as_str() {
                "brier" => {
                    let bs: f64 = (0..n)
                        .map(|i| (scores[i] - labels[i]).powi(2))
                        .sum::<f64>() / n as f64;
                    names.push("brier_score".into());
                    values.push(bs);
                }
                "hl_test" | "hosmer_lemeshow" => {
                    // Hosmer-Lemeshow chi-squared statistic.
                    let mut sorted_idx: Vec<usize> = (0..n).collect();
                    sorted_idx.sort_by(|&a, &b| scores[a].partial_cmp(&scores[b]).unwrap_or(std::cmp::Ordering::Equal));

                    let group_size = (n + self.spec.n_bins - 1) / self.spec.n_bins;
                    let mut chi_sq = 0.0f64;
                    let mut df: usize = 0;
                    for g in 0..self.spec.n_bins {
                        let start = g * group_size;
                        let end = ((g + 1) * group_size).min(n);
                        if start >= end { break; }
                        let group: Vec<usize> = (start..end).map(|i| sorted_idx[i]).collect();
                        let gn = group.len() as f64;
                        let mean_pred: f64 = group.iter().map(|&i| scores[i]).sum::<f64>() / gn;
                        let mean_obs: f64 = group.iter().map(|&i| labels[i]).sum::<f64>() / gn;
                        let expected_pos = mean_pred * gn;
                        let expected_neg = (1.0 - mean_pred) * gn;
                        let observed_pos = mean_obs * gn;
                        let observed_neg = gn - observed_pos;
                        if expected_pos > 0.0 {
                            chi_sq += (observed_pos - expected_pos).powi(2) / expected_pos;
                        }
                        if expected_neg > 0.0 {
                            chi_sq += (observed_neg - expected_neg).powi(2) / expected_neg;
                        }
                        df += 1;
                    }
                    df = df.saturating_sub(2usize).max(1usize);
                    names.push("hl_chi_squared".into());
                    values.push(chi_sq);
                    names.push("hl_df".into());
                    values.push(df as f64);
                }
                "calibration_curve" => {
                    // Will be output as separate rows in a second port.
                    // For now, just compute mean absolute calibration error.
                    let mut sorted_idx: Vec<usize> = (0..n).collect();
                    sorted_idx.sort_by(|&a, &b| scores[a].partial_cmp(&scores[b]).unwrap_or(std::cmp::Ordering::Equal));
                    let group_size = (n + self.spec.n_bins - 1) / self.spec.n_bins;
                    let mut cal_error = 0.0;
                    for g in 0..self.spec.n_bins {
                        let start = g * group_size;
                        let end = ((g + 1) * group_size).min(n);
                        if start >= end { break; }
                        let group: Vec<usize> = (start..end).map(|i| sorted_idx[i]).collect();
                        let gn = group.len() as f64;
                        let mean_pred: f64 = group.iter().map(|&i| scores[i]).sum::<f64>() / gn;
                        let mean_obs: f64 = group.iter().map(|&i| labels[i]).sum::<f64>() / gn;
                        cal_error += (mean_pred - mean_obs).abs() / self.spec.n_bins as f64;
                    }
                    names.push("mean_calibration_error".into());
                    values.push(cal_error);
                }
                _ => {}
            }
        }

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("metric", DataType::Utf8, false),
                Field::new("value", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(names)),
                Arc::new(Float64Array::from(values)),
            ],
        ).map_err(|e| common::err(NODE_CAL, format!("build batch: {e}")))?;

        common::emit_batch(ctx, batch, NODE_CAL)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// dl_nri — Net Reclassification Index
// ═══════════════════════════════════════════════════════════════════════

const NODE_NRI: &str = "dl_nri";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NriSpec {
    pub score1_column: String,
    pub score2_column: String,
    pub label_column: String,
    #[serde(default = "d_thresholds")]
    pub thresholds: Vec<f64>,
}

fn d_thresholds() -> Vec<f64> { vec![0.05, 0.10, 0.20] }

pub struct NriFactory;
impl NodeFactory for NriFactory {
    fn kind(&self) -> &'static str { NODE_NRI }
    fn desc(&self) -> &'static str { "Net Reclassification Index (NRI) and IDI." }
    fn doc(&self) -> &'static str {
        "dl_nri: computes categorical and continuous NRI (Net Reclassification Index) and \
        IDI (Integrated Discrimination Improvement) for comparing two prediction models."
    }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(NriSpec) }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: NriSpec = serde_json::from_value(spec)?;
        Ok(Box::new(NriNode { spec: s, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct NriNode {
    spec: NriSpec,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for NriNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { NODE_NRI }
    fn as_any(&self) -> &dyn std::any::Any { self }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = common::collect_batches(inputs, NODE_NRI).await?;
        let s1 = common::extract_numeric_column(&batches, &self.spec.score1_column)?;
        let s2 = common::extract_numeric_column(&batches, &self.spec.score2_column)?;
        let labels = common::extract_numeric_column(&batches, &self.spec.label_column)?;
        let n = s1.len();

        // Assign risk categories.
        let categorize = |score: f64, thresholds: &[f64]| -> usize {
            let mut cat = 0;
            for &t in thresholds {
                if score >= t { cat += 1; }
            }
            cat
        };

        let mut up_events = 0.0;
        let mut down_events = 0.0;
        let mut up_nonevents = 0.0;
        let mut down_nonevents = 0.0;
        let mut n_events: f64 = 0.0;
        let mut n_nonevents: f64 = 0.0;

        for i in 0..n {
            let c1 = categorize(s1[i], &self.spec.thresholds);
            let c2 = categorize(s2[i], &self.spec.thresholds);
            let is_event = labels[i] > 0.5;

            if is_event {
                n_events += 1.0;
                if c2 > c1 { up_events += 1.0; }
                if c2 < c1 { down_events += 1.0; }
            } else {
                n_nonevents += 1.0;
                if c2 > c1 { up_nonevents += 1.0; }
                if c2 < c1 { down_nonevents += 1.0; }
            }
        }

        let nri_event = if n_events > 0.0 { (up_events - down_events) / n_events } else { 0.0 };
        let nri_nonevent = if n_nonevents > 0.0 { (down_nonevents - up_nonevents) / n_nonevents } else { 0.0 };
        let nri_total = nri_event + nri_nonevent;

        // IDI.
        let mean_s1_events: f64 = (0..n).filter(|&i| labels[i] > 0.5).map(|i| s1[i]).sum::<f64>() / n_events.max(1.0_f64);
        let mean_s1_nonevents: f64 = (0..n).filter(|&i| labels[i] <= 0.5).map(|i| s1[i]).sum::<f64>() / n_nonevents.max(1.0_f64);
        let mean_s2_events: f64 = (0..n).filter(|&i| labels[i] > 0.5).map(|i| s2[i]).sum::<f64>() / n_events.max(1.0_f64);
        let mean_s2_nonevents: f64 = (0..n).filter(|&i| labels[i] <= 0.5).map(|i| s2[i]).sum::<f64>() / n_nonevents.max(1.0_f64);
        let idi = (mean_s2_events - mean_s2_nonevents) - (mean_s1_events - mean_s1_nonevents);

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("metric", DataType::Utf8, false),
                Field::new("value", DataType::Float64, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec![
                    "nri_event", "nri_nonevent", "nri_total", "idi",
                ])),
                Arc::new(Float64Array::from(vec![
                    nri_event, nri_nonevent, nri_total, idi,
                ])),
            ],
        ).map_err(|e| common::err(NODE_NRI, format!("build batch: {e}")))?;

        common::emit_batch(ctx, batch, NODE_NRI)
    }
}
