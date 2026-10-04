//! Donor-level single-cell composition test.
//!
//! Cells are first aggregated to donor-by-cell-type counts. Each cell type is
//! then tested with a binomial logistic model (`cell count / donor total`
//! versus condition), so cells are never treated as independent biological
//! replicates.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{dag::DagError, dag::graph::PortOutputs};

use crate::common::{HypoNodeError, collect_input};

pub const DONOR_COMPOSITION_KIND: &str = "hypothesize.donor_composition_test";

fn default_min_donor_cells() -> u64 {
    10
}

fn default_correction() -> String {
    "BH".into()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DonorCompositionSpec {
    pub donor_col: String,
    pub cell_type_col: String,
    pub condition_col: String,
    pub reference: String,
    pub test: String,
    #[serde(default = "default_min_donor_cells")]
    pub min_donor_cells: u64,
    #[serde(default = "default_correction")]
    pub correction: String,
}

pub struct DonorCompositionNodeFactory;

#[derive(Clone)]
pub struct DonorCompositionNode {
    ports: NodePorts,
    spec: DonorCompositionSpec,
}

fn validate(spec: &DonorCompositionSpec) -> Result<(), String> {
    for (name, value) in [
        ("donor_col", &spec.donor_col),
        ("cell_type_col", &spec.cell_type_col),
        ("condition_col", &spec.condition_col),
        ("reference", &spec.reference),
        ("test", &spec.test),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{name} cannot be empty"));
        }
    }
    if spec.reference == spec.test {
        return Err("reference and test conditions must differ".into());
    }
    if spec.min_donor_cells == 0 {
        return Err("min_donor_cells must be greater than zero".into());
    }
    Ok(())
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(None)
        .add_output_port(None)
}

impl NodeFactory for DonorCompositionNodeFactory {
    fn kind(&self) -> &'static str {
        DONOR_COMPOSITION_KIND
    }
    fn desc(&self) -> &'static str {
        "Tests donor-level cell-type composition without treating cells as replicates."
    }
    fn doc(&self) -> &'static str {
        "Input is a cell-level table with donor, cell-type, and condition columns. The node aggregates donor-by-cell-type counts, fits a donor-level binomial logistic model per cell type, reports test-versus-reference log-odds, Wald z, raw p, and multiplicity-adjusted p, and also emits the donor composition table used by the model."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(DonorCompositionSpec)
    }
    fn ports(&self) -> NodePorts {
        port_layout()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: DonorCompositionSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(Box::new(DonorCompositionNode {
            ports: port_layout(),
            spec,
        }))
    }
}

#[derive(Clone)]
struct DonorCompositionNodeInner {
    meta: NodePorts,
    spec: DonorCompositionSpec,
}

impl DonorCompositionNode {
    fn inner(&self) -> DonorCompositionNodeInner {
        DonorCompositionNodeInner {
            meta: self.ports.clone(),
            spec: self.spec.clone(),
        }
    }
}

#[async_trait]
impl DagNode for DonorCompositionNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        DONOR_COMPOSITION_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        self.inner().execute(ctx, inputs, reporter).await
    }
}

#[async_trait]
impl DagNode for DonorCompositionNodeInner {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        DONOR_COMPOSITION_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_input(inputs).await.map_err(DagError::from)?;
        let donors = extract_triple(&batches, &self.spec)?;
        let tests = fit_composition(&donors, &self.spec)?;
        let composition =
            composition_batch(&donors).map_err(|e| DagError::from(HypoNodeError::Spec(e)))?;
        let test_batch = test_batch(&tests).map_err(|e| DagError::from(HypoNodeError::Spec(e)))?;
        let session = ctx.session();
        let composition_df = session
            .read_batch(composition)
            .map_err(|e| DagError::from(HypoNodeError::ReadBatch(e.to_string())))?;
        let test_df = session
            .read_batch(test_batch)
            .map_err(|e| DagError::from(HypoNodeError::ReadBatch(e.to_string())))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, test_df);
        outputs.insert(1, composition_df);
        Ok(outputs)
    }
}

struct DonorTable {
    donors: BTreeMap<String, String>,
    totals: BTreeMap<String, u64>,
    counts: BTreeMap<(String, String), u64>,
    cell_types: BTreeSet<String>,
}

struct CompositionResult {
    cell_type: String,
    donors: u64,
    cells: u64,
    reference_proportion: f64,
    test_proportion: f64,
    log_odds_ratio: f64,
    standard_error: f64,
    z: f64,
    p_raw: f64,
    p_adj: f64,
}

fn string_column(batch: &RecordBatch, name: &str) -> Result<Vec<Option<String>>, HypoNodeError> {
    let values = batch
        .column_by_name(name)
        .and_then(|array| array.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| HypoNodeError::Column(name.to_string()))?;
    Ok((0..values.len())
        .map(|row| (!values.is_null(row)).then(|| values.value(row).to_string()))
        .collect())
}

fn extract_triple(
    batches: &[RecordBatch],
    spec: &DonorCompositionSpec,
) -> Result<DonorTable, HypoNodeError> {
    let mut table = DonorTable {
        donors: BTreeMap::new(),
        totals: BTreeMap::new(),
        counts: BTreeMap::new(),
        cell_types: BTreeSet::new(),
    };
    for batch in batches {
        let donors = string_column(batch, &spec.donor_col)?;
        let cell_types = string_column(batch, &spec.cell_type_col)?;
        let conditions = string_column(batch, &spec.condition_col)?;
        if donors.len() != cell_types.len() || donors.len() != conditions.len() {
            return Err(HypoNodeError::Column("column length mismatch".into()));
        }
        for row in 0..batch.num_rows() {
            let donor = donors[row]
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty());
            let cell_type = cell_types[row]
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty());
            let condition = conditions[row]
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty());
            let (Some(donor), Some(cell_type), Some(condition)) = (donor, cell_type, condition)
            else {
                return Err(HypoNodeError::Column(
                    "donor/cell_type/condition cannot be null or empty".into(),
                ));
            };
            if condition != spec.reference && condition != spec.test {
                return Err(HypoNodeError::Spec(format!(
                    "unexpected condition `{condition}`; expected {} or {}",
                    spec.reference, spec.test
                )));
            }
            match table.donors.get(donor) {
                Some(previous) if previous != condition => {
                    return Err(HypoNodeError::Spec(format!(
                        "donor `{donor}` maps to multiple conditions"
                    )));
                }
                _ => {}
            }
            table
                .donors
                .insert(donor.to_string(), condition.to_string());
            *table.totals.entry(donor.to_string()).or_default() += 1;
            *table
                .counts
                .entry((donor.to_string(), cell_type.to_string()))
                .or_default() += 1;
            table.cell_types.insert(cell_type.to_string());
        }
    }
    if table.donors.len() < 2 {
        return Err(HypoNodeError::Insufficient(
            "at least two donors are required".into(),
        ));
    }
    if !table.donors.values().any(|v| v == &spec.reference)
        || !table.donors.values().any(|v| v == &spec.test)
    {
        return Err(HypoNodeError::Insufficient(
            "both reference and test donors are required".into(),
        ));
    }
    Ok(table)
}

fn fit_composition(
    table: &DonorTable,
    spec: &DonorCompositionSpec,
) -> Result<Vec<CompositionResult>, HypoNodeError> {
    let mut results = Vec::new();
    for cell_type in &table.cell_types {
        let mut observations = Vec::new();
        for (donor, condition) in &table.donors {
            let total = table.totals[donor];
            if total >= spec.min_donor_cells {
                let count = table
                    .counts
                    .get(&(donor.clone(), cell_type.clone()))
                    .copied()
                    .unwrap_or_default();
                observations.push((
                    if condition == &spec.test { 1.0 } else { 0.0 },
                    count as f64,
                    total as f64,
                ));
            }
        }
        if observations.len() < 2 {
            continue;
        }
        let Some((log_odds_ratio, standard_error, z, p_raw)) = fit_binomial(&observations) else {
            continue;
        };
        let ref_values: Vec<_> = observations
            .iter()
            .filter(|(x, _, _)| *x == 0.0)
            .map(|(_, k, n)| k / n)
            .collect();
        let test_values: Vec<_> = observations
            .iter()
            .filter(|(x, _, _)| *x == 1.0)
            .map(|(_, k, n)| k / n)
            .collect();
        results.push(CompositionResult {
            cell_type: cell_type.clone(),
            donors: observations.len() as u64,
            cells: observations.iter().map(|(_, k, _)| *k as u64).sum(),
            reference_proportion: ref_values.iter().sum::<f64>() / ref_values.len() as f64,
            test_proportion: test_values.iter().sum::<f64>() / test_values.len() as f64,
            log_odds_ratio,
            standard_error,
            z,
            p_raw,
            p_adj: p_raw,
        });
    }
    let pvalues = results.iter().map(|row| row.p_raw).collect::<Vec<_>>();
    if !pvalues.is_empty() {
        let method = hypothesize::AdjustMethod::parse(&spec.correction)
            .map_err(|error| HypoNodeError::Spec(error.to_string()))?;
        let adjusted = hypothesize::p_adjust_raw(&pvalues, method, Some(pvalues.len()))
            .map_err(|error| HypoNodeError::Test(error.to_string()))?;
        results
            .iter_mut()
            .zip(adjusted)
            .for_each(|(row, p)| row.p_adj = p);
    }
    Ok(results)
}

fn fit_binomial(observations: &[(f64, f64, f64)]) -> Option<(f64, f64, f64, f64)> {
    let overall = observations.iter().map(|(_, k, _n)| k).sum::<f64>()
        / observations.iter().map(|(_, _, n)| n).sum::<f64>();
    let mut intercept = (overall / (1.0 - overall)).ln();
    let mut slope = 0.0;
    for _ in 0..100 {
        let mut h = [[0.0, 0.0], [0.0, 0.0]];
        let mut score = [0.0, 0.0];
        for &(x, successes, trials) in observations {
            let probability = (intercept + slope * x).sigmoid();
            let weight = trials * probability * (1.0 - probability).max(1e-10);
            let residual = successes - trials * probability;
            h[0][0] += weight;
            h[0][1] += weight * x;
            h[1][0] += weight * x;
            h[1][1] += weight * x * x;
            score[0] += residual;
            score[1] += residual * x;
        }
        let determinant = h[0][0] * h[1][1] - h[0][1] * h[1][0];
        if !determinant.is_finite() || determinant.abs() < 1e-14 {
            return None;
        }
        let delta = [
            (h[1][1] * score[0] - h[0][1] * score[1]) / determinant,
            (-h[1][0] * score[0] + h[0][0] * score[1]) / determinant,
        ];
        intercept += delta[0];
        slope += delta[1];
        if delta.iter().all(|value| value.abs() < 1e-10) {
            break;
        }
    }
    let mut h = [[0.0, 0.0], [0.0, 0.0]];
    for &(x, _, trials) in observations {
        let probability = (intercept + slope * x).sigmoid();
        let weight = trials * probability * (1.0 - probability).max(1e-10);
        h[0][0] += weight;
        h[0][1] += weight * x;
        h[1][0] += weight * x;
        h[1][1] += weight * x * x;
    }
    let determinant = h[0][0] * h[1][1] - h[0][1] * h[1][0];
    if !determinant.is_finite() || determinant.abs() < 1e-14 {
        return None;
    }
    // theta = [intercept, slope]; H⁻¹ = adj(H)/det, so Var(slope) = (H⁻¹)₁₁
    // = h[0][0]/det (previously h[1][1]/det, which is Var(intercept) and
    // understated the slope SE).
    let covariance = h[0][0] / determinant;
    if !covariance.is_finite() || covariance <= 0.0 {
        return None;
    }
    let standard_error = covariance.sqrt();
    let z = slope / standard_error;
    let p_raw = 2.0 * hypothesize::normal_cdf(-z.abs());
    Some((slope, standard_error, z, p_raw))
}

trait Sigmoid {
    fn sigmoid(self) -> Self;
}

impl Sigmoid for f64 {
    fn sigmoid(self) -> Self {
        if self >= 0.0 {
            1.0 / (1.0 + (-self).exp())
        } else {
            self.exp() / (1.0 + self.exp())
        }
    }
}

fn composition_batch(table: &DonorTable) -> Result<RecordBatch, String> {
    let mut donors = Vec::new();
    let mut conditions = Vec::new();
    let mut cell_types = Vec::new();
    let mut counts = Vec::new();
    let mut totals = Vec::new();
    let mut proportions = Vec::new();
    for donor in table.donors.keys() {
        for cell_type in &table.cell_types {
            let count = table
                .counts
                .get(&(donor.clone(), cell_type.clone()))
                .copied()
                .unwrap_or_default();
            let total = table.totals[donor];
            donors.push(donor.clone());
            conditions.push(table.donors[donor].clone());
            cell_types.push(cell_type.clone());
            counts.push(count);
            totals.push(total);
            proportions.push(count as f64 / total as f64);
        }
    }
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("donor_id", DataType::Utf8, false),
            Field::new("condition", DataType::Utf8, false),
            Field::new("cell_type", DataType::Utf8, false),
            Field::new("count", DataType::UInt64, false),
            Field::new("donor_total", DataType::UInt64, false),
            Field::new("proportion", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(donors)),
            Arc::new(StringArray::from(conditions)),
            Arc::new(StringArray::from(cell_types)),
            Arc::new(UInt64Array::from(counts)),
            Arc::new(UInt64Array::from(totals)),
            Arc::new(Float64Array::from(proportions)),
        ],
    )
    .map_err(|error| error.to_string())
}

fn test_batch(rows: &[CompositionResult]) -> Result<RecordBatch, String> {
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("cell_type", DataType::Utf8, false),
            Field::new("donors", DataType::UInt64, false),
            Field::new("cells", DataType::UInt64, false),
            Field::new("reference_proportion", DataType::Float64, false),
            Field::new("test_proportion", DataType::Float64, false),
            Field::new("log_odds_ratio", DataType::Float64, false),
            Field::new("standard_error", DataType::Float64, false),
            Field::new("z", DataType::Float64, false),
            Field::new("p_raw", DataType::Float64, false),
            Field::new("p_adj", DataType::Float64, false),
        ])),
        vec![
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|row| row.cell_type.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                rows.iter().map(|row| row.donors).collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                rows.iter().map(|row| row.cells).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|row| row.reference_proportion)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|row| row.test_proportion)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|row| row.log_odds_ratio)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|row| row.standard_error)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.z).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.p_raw).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|row| row.p_adj).collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch() -> RecordBatch {
        let mut donors = Vec::new();
        let mut conditions = Vec::new();
        let mut cell_types = Vec::new();
        for donor in 0..8 {
            let condition = if donor < 4 { "control" } else { "case" };
            for _ in 0..20 {
                donors.push(format!("d{donor}"));
                conditions.push(condition);
                // Cases enrich for T cells; controls enrich for B cells.
                let cell_type = if condition == "case" {
                    if donors.len() % 4 == 0 { "T" } else { "B" }
                } else if donors.len() % 4 == 0 {
                    "B"
                } else {
                    "T"
                };
                cell_types.push(cell_type);
            }
        }
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("donor_id", DataType::Utf8, false),
                Field::new("condition", DataType::Utf8, false),
                Field::new("cell_type", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(StringArray::from(donors)),
                Arc::new(StringArray::from(conditions)),
                Arc::new(StringArray::from(cell_types)),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn fits_donor_level_binomial_models() {
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let mut node = DonorCompositionNodeFactory
            .build(
                serde_json::json!({
                    "donor_col": "donor_id",
                    "cell_type_col": "cell_type",
                    "condition_col": "condition",
                    "reference": "control",
                    "test": "case"
                }),
                ctx.clone(),
            )
            .unwrap();
        let dataframe = ctx.session().read_batch(batch()).unwrap();
        let outputs = node
            .execute(
                &ctx,
                &[NodeInput::new_dataframe(0, dataframe)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let tests = outputs
            .get(&0)
            .unwrap()
            .as_dataframe()
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(tests.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);
        let composition = outputs
            .get(&1)
            .unwrap()
            .as_dataframe()
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(
            composition.iter().map(RecordBatch::num_rows).sum::<usize>(),
            16
        );
    }
}
