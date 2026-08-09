//! Preprocessing DAG nodes — scalers, encoders, imputers, transforms.
//!
//! Each node reads numeric columns from the input port, applies a transform,
//! and outputs the transformed table. Fitted scaler parameters are available
//! as an optional model-artifact output port when `emit_model = true`.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use faer::Mat;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use ml::preprocess::{
    ImputeStrategy, Imputer, MinMaxScaler, NormKind, PowerTransformer, RobustScaler,
    StandardScaler, normalize_rows,
};

// ── helper: extract feature matrix from input ───────────────────────────

fn extract_features(batches: &[RecordBatch], columns: &[String]) -> Result<Mat<f64>, DagError> {
    common::extract_matrix(batches, columns).map_err(|e| DagError::NodeError {
        node_type: "ml_preprocess".into(),
        msg: e.to_string(),
    })
}

/// Replace the given columns in a batch with transformed values, preserving
/// all other columns.
fn replace_columns(
    batches: &[RecordBatch],
    replace: &[String],
    new_data: &Mat<f64>,
    extra_cols: &[(&str, Vec<f64>)],
) -> Result<RecordBatch, DagError> {
    let schema = batches.first().ok_or(DagError::NodeError {
        node_type: "ml_preprocess".into(),
        msg: "no input rows".into(),
    })?;
    let orig_schema = schema.schema();
    let n_rows = new_data.nrows();

    let mut fields: Vec<(Arc<Field>, Vec<Arc<dyn Array>>)> = Vec::new();

    // Re-emit original columns that are NOT in `replace`
    let replace_set: std::collections::HashSet<&str> = replace.iter().map(|s| s.as_str()).collect();

    for (col_i, field) in orig_schema.fields().iter().enumerate() {
        if replace_set.contains(field.name().as_str()) {
            continue;
        }
        let mut col_values: Vec<Arc<dyn Array>> = Vec::new();
        for batch in batches {
            col_values.push(batch.column(col_i).clone());
        }
        fields.push((field.clone(), col_values));
    }

    // Add transformed columns
    let (nrows, _ncols) = new_data.shape();
    let mut all_fields: Vec<Arc<Field>> = fields.iter().map(|(f, _)| f.clone()).collect();
    let mut all_arrays: Vec<Arc<dyn Array>> =
        fields.into_iter().flat_map(|(_, arrays)| arrays).collect();

    for (j, name) in replace.iter().enumerate() {
        let col_data: Vec<f64> = (0..nrows).map(|i| new_data[(i, j)]).collect();
        all_fields.push(Arc::new(Field::new(name, DataType::Float64, true)));
        all_arrays.push(Arc::new(Float64Array::from(col_data)));
    }

    for (name, values) in extra_cols {
        all_fields.push(Arc::new(Field::new(*name, DataType::Float64, true)));
        all_arrays.push(Arc::new(Float64Array::from(values.clone())));
    }

    let _ = n_rows;
    RecordBatch::try_new(Arc::new(Schema::new(all_fields)), all_arrays).map_err(|e| {
        DagError::NodeError {
            node_type: "ml_preprocess".into(),
            msg: format!("failed to build output batch: {e}"),
        }
    })
}

// ═══════════════════════════════════════════════════════════════════════
// Standardize
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StandardizeSpec {
    /// Numeric columns to standardise (z-score).
    pub columns: Vec<String>,
}

pub struct StandardizeFactory;
impl NodeFactory for StandardizeFactory {
    fn kind(&self) -> &'static str {
        "ml_standardize"
    }
    fn desc(&self) -> &'static str {
        "Standardise numeric columns (z-score: subtract mean, divide by std)."
    }
    fn doc(&self) -> &'static str {
        "StandardScaler: for each column, subtract the mean and divide by standard deviation. Constant columns (std=0) cause an error."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StandardizeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: StandardizeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(StandardizeNode {
            columns: s.columns,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct StandardizeNode {
    columns: Vec<String>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for StandardizeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_standardize"
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
        let (_, transformed) =
            StandardScaler::fit_transform(&data).map_err(|e| DagError::NodeError {
                node_type: "ml_standardize".into(),
                msg: e.to_string(),
            })?;
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// MinMaxScale
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MinMaxScaleSpec {
    pub columns: Vec<String>,
    #[serde(default = "default_lo")]
    pub feature_min: f64,
    #[serde(default = "default_hi")]
    pub feature_max: f64,
}
fn default_lo() -> f64 {
    0.0
}
fn default_hi() -> f64 {
    1.0
}

pub struct MinMaxScaleFactory;
impl NodeFactory for MinMaxScaleFactory {
    fn kind(&self) -> &'static str {
        "ml_minmax_scale"
    }
    fn desc(&self) -> &'static str {
        "Scale numeric columns to a fixed range [min, max]."
    }
    fn doc(&self) -> &'static str {
        "MinMaxScaler: linearly scales each column to [feature_min, feature_max]. Default range is [0, 1]."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MinMaxScaleSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: MinMaxScaleSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MinMaxScaleNode {
            columns: s.columns,
            feature_range: (s.feature_min, s.feature_max),
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct MinMaxScaleNode {
    columns: Vec<String>,
    feature_range: (f64, f64),
    meta: NodePorts,
}

#[async_trait]
impl DagNode for MinMaxScaleNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_minmax_scale"
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
        let (_, mut transformed) =
            MinMaxScaler::fit_transform(&data, self.feature_range).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_minmax_scale".into(),
                    msg: e.to_string(),
                }
            })?;
        let _ = &mut transformed;
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// RobustScale
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RobustScaleSpec {
    pub columns: Vec<String>,
}

pub struct RobustScaleFactory;
impl NodeFactory for RobustScaleFactory {
    fn kind(&self) -> &'static str {
        "ml_robust_scale"
    }
    fn desc(&self) -> &'static str {
        "Scale using median and IQR (robust to outliers)."
    }
    fn doc(&self) -> &'static str {
        "RobustScaler: subtract median and divide by interquartile range (Q3 - Q1). Outlier-resistant."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RobustScaleSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: RobustScaleSpec = serde_json::from_value(spec)?;
        Ok(Box::new(RobustScaleNode {
            columns: s.columns,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct RobustScaleNode {
    columns: Vec<String>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for RobustScaleNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_robust_scale"
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
        let scaler = RobustScaler::fit(&data).map_err(|e| DagError::NodeError {
            node_type: "ml_robust_scale".into(),
            msg: e.to_string(),
        })?;
        let mut transformed = data;
        scaler.transform(&mut transformed);
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// NormalizeRows
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NormalizeRowsSpec {
    pub columns: Vec<String>,
    #[serde(default = "default_norm")]
    pub norm: String,
}
fn default_norm() -> String {
    "l2".into()
}

pub struct NormalizeRowsFactory;
impl NodeFactory for NormalizeRowsFactory {
    fn kind(&self) -> &'static str {
        "ml_normalize_rows"
    }
    fn desc(&self) -> &'static str {
        "Normalise each row to unit norm (L1, L2, or Max)."
    }
    fn doc(&self) -> &'static str {
        "Row-wise normalisation: each row vector is scaled so its L1/L2/Max norm equals 1."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(NormalizeRowsSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: NormalizeRowsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(NormalizeRowsNode {
            columns: s.columns,
            norm: s.norm,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct NormalizeRowsNode {
    columns: Vec<String>,
    norm: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for NormalizeRowsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_normalize_rows"
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
        let mut data = extract_features(&batches, &self.columns)?;
        let kind = match self.norm.to_lowercase().as_str() {
            "l1" => NormKind::L1,
            "l2" => NormKind::L2,
            "max" => NormKind::Max,
            _ => NormKind::L2,
        };
        normalize_rows(&mut data, kind);
        let batch = replace_columns(&batches, &self.columns, &data, &[])?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PowerTransform
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PowerTransformSpec {
    pub columns: Vec<String>,
}

pub struct PowerTransformFactory;
impl NodeFactory for PowerTransformFactory {
    fn kind(&self) -> &'static str {
        "ml_power_transform"
    }
    fn desc(&self) -> &'static str {
        "Yeo-Johnson power transform for approximate normality."
    }
    fn doc(&self) -> &'static str {
        "PowerTransformer: fits Yeo-Johnson λ per column via MLE, transforms data to approximate Gaussian. Works on positive and negative values."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerTransformSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PowerTransformSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerTransformNode {
            columns: s.columns,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct PowerTransformNode {
    columns: Vec<String>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for PowerTransformNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_power_transform"
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
        let scaler = PowerTransformer::fit(&data).map_err(|e| DagError::NodeError {
            node_type: "ml_power_transform".into(),
            msg: e.to_string(),
        })?;
        let mut transformed = data;
        scaler.transform(&mut transformed);
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Impute
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ImputeSpec {
    pub columns: Vec<String>,
    #[serde(default = "default_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub fill_value: Option<f64>,
}
fn default_strategy() -> String {
    "mean".into()
}

pub struct ImputeFactory;
impl NodeFactory for ImputeFactory {
    fn kind(&self) -> &'static str {
        "ml_impute"
    }
    fn desc(&self) -> &'static str {
        "Replace NaN values with mean, median, or a constant."
    }
    fn doc(&self) -> &'static str {
        "Imputer: fills NaN values in specified columns using mean, median, or constant fill_value strategy."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ImputeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ImputeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ImputeNode {
            columns: s.columns,
            strategy: s.strategy,
            fill_value: s.fill_value,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ImputeNode {
    columns: Vec<String>,
    strategy: String,
    fill_value: Option<f64>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ImputeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_impute"
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
        let strat = match self.strategy.as_str() {
            "median" => ImputeStrategy::Median,
            "constant" => ImputeStrategy::Constant(self.fill_value.unwrap_or(0.0)),
            _ => ImputeStrategy::Mean,
        };
        let imputer = Imputer::fit(&data, strat).map_err(|e| DagError::NodeError {
            node_type: "ml_impute".into(),
            msg: e.to_string(),
        })?;
        let mut transformed = data;
        imputer.transform(&mut transformed);
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// OneHotEncode
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OneHotEncodeSpec {
    pub column: String,
    #[serde(default)]
    pub categories: Option<Vec<String>>,
}

pub struct OneHotEncodeFactory;
impl NodeFactory for OneHotEncodeFactory {
    fn kind(&self) -> &'static str {
        "ml_one_hot"
    }
    fn desc(&self) -> &'static str {
        "One-hot encode a categorical column into binary indicator columns."
    }
    fn doc(&self) -> &'static str {
        "OneHotEncoder: expands a categorical column into one binary column per category. If categories are not specified, they are inferred from the data."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OneHotEncodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: OneHotEncodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OneHotEncodeNode {
            column: s.column,
            categories: s.categories,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct OneHotEncodeNode {
    column: String,
    categories: Option<Vec<String>>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for OneHotEncodeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_one_hot"
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
        let values = common::extract_string_column(&batches, &self.column)?;

        // Determine categories
        let categories: Vec<String> = match &self.categories {
            Some(c) => c.clone(),
            None => {
                let mut seen: Vec<String> = values.to_vec();
                seen.sort();
                seen.dedup();
                seen
            }
        };

        // Build indicator columns
        let mut fields: Vec<Arc<Field>> = Vec::new();
        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();

        // Re-emit all original columns except the encoded one
        let schema = batches.first().unwrap().schema();
        for (i, f) in schema.fields().iter().enumerate() {
            if f.name() == &self.column {
                continue;
            }
            fields.push(f.clone());
            let mut col_chunks: Vec<Arc<dyn Array>> = Vec::new();
            for batch in &batches {
                col_chunks.push(batch.column(i).clone());
            }
            // Concatenate chunks
            arrays.push(col_chunks.into_iter().next().unwrap());
        }

        // Add one-hot columns
        for cat in &categories {
            let indicator: Vec<f64> = values
                .iter()
                .map(|v| if v.as_str() == cat.as_str() { 1.0 } else { 0.0 })
                .collect();
            let col_name = format!("{}_{}", self.column, cat);
            fields.push(Arc::new(Field::new(&col_name, DataType::Float64, true)));
            arrays.push(Arc::new(Float64Array::from(indicator)));
        }

        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_one_hot".into(),
                msg: e.to_string(),
            }
        })?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// LabelEncode
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LabelEncodeSpec {
    pub column: String,
}

pub struct LabelEncodeFactory;
impl NodeFactory for LabelEncodeFactory {
    fn kind(&self) -> &'static str {
        "ml_label_encode"
    }
    fn desc(&self) -> &'static str {
        "Encode a categorical column as integer labels [0, n_classes)."
    }
    fn doc(&self) -> &'static str {
        "LabelEncoder: maps each unique value in a categorical column to an integer starting from 0."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LabelEncodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: LabelEncodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LabelEncodeNode {
            column: s.column,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct LabelEncodeNode {
    column: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for LabelEncodeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_label_encode"
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
        let values = common::extract_string_column(&batches, &self.column)?;
        // Build label map
        let mut sorted_cats: Vec<String> = values.to_vec();
        sorted_cats.sort();
        sorted_cats.dedup();
        let map: std::collections::HashMap<&String, u32> = sorted_cats
            .iter()
            .enumerate()
            .map(|(i, c)| (c, i as u32))
            .collect();
        let encoded: Vec<u32> = values.iter().map(|v| *map.get(v).unwrap_or(&0)).collect();

        let schema = batches.first().unwrap().schema();
        let mut fields: Vec<Arc<Field>> = Vec::new();
        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();
        for (i, f) in schema.fields().iter().enumerate() {
            if f.name() == &self.column {
                fields.push(Arc::new(Field::new(&self.column, DataType::UInt32, true)));
                arrays.push(Arc::new(UInt32Array::from(encoded.clone())));
            } else {
                fields.push(f.clone());
                arrays.push(batches.first().unwrap().column(i).clone());
            }
        }
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_label_encode".into(),
                msg: e.to_string(),
            }
        })?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// PolyFeatures
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PolyFeaturesSpec {
    pub columns: Vec<String>,
    #[serde(default = "default_degree")]
    pub degree: usize,
    #[serde(default = "default_true")]
    pub include_bias: bool,
    #[serde(default = "default_true")]
    pub interaction_only: bool,
}
fn default_degree() -> usize {
    2
}
fn default_true() -> bool {
    true
}

pub struct PolyFeaturesFactory;
impl NodeFactory for PolyFeaturesFactory {
    fn kind(&self) -> &'static str {
        "ml_poly_features"
    }
    fn desc(&self) -> &'static str {
        "Generate polynomial and interaction features."
    }
    fn doc(&self) -> &'static str {
        "PolynomialFeatures: generates all polynomial combinations of features up to the specified degree. Optionally interaction-only (no x^2 terms)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PolyFeaturesSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PolyFeaturesSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PolyFeaturesNode {
            columns: s.columns,
            degree: s.degree,
            include_bias: s.include_bias,
            interaction_only: s.interaction_only,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct PolyFeaturesNode {
    columns: Vec<String>,
    degree: usize,
    include_bias: bool,
    interaction_only: bool,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for PolyFeaturesNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_poly_features"
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

        // Generate combinations
        let combos =
            polynomial_combinations(ncols, self.degree, self.include_bias, self.interaction_only);
        let n_out = combos.len();

        let mut out_data = vec![0.0f64; nrows * n_out];
        for (out_j, combo) in combos.iter().enumerate() {
            for i in 0..nrows {
                let mut val = 1.0;
                for &col in combo {
                    val *= data[(i, col)];
                }
                out_data[i * n_out + out_j] = val;
            }
        }
        let result_mat = crate::common::mat_from_row_major(nrows, n_out, &out_data);

        // Output column names
        let names: Vec<String> = combos
            .iter()
            .map(|combo| {
                if combo.is_empty() {
                    "bias".into()
                } else {
                    combo
                        .iter()
                        .map(|&c| self.columns[c].as_str())
                        .collect::<Vec<_>>()
                        .join(":")
                }
            })
            .collect();

        let batch = replace_columns_with_names(&batches, &names, &result_mat)?;
        emit_batch(ctx, batch)
    }
}

fn polynomial_combinations(
    n_features: usize,
    degree: usize,
    include_bias: bool,
    interaction_only: bool,
) -> Vec<Vec<usize>> {
    fn recurse(
        start: usize,
        n_features: usize,
        depth: usize,
        max_depth: usize,
        interaction_only: bool,
        current: &mut Vec<usize>,
        out: &mut Vec<Vec<usize>>,
    ) {
        if depth == max_depth {
            out.push(current.clone());
            return;
        }
        for i in start..n_features {
            current.push(i);
            recurse(
                if interaction_only { i + 1 } else { i },
                n_features,
                depth + 1,
                max_depth,
                interaction_only,
                current,
                out,
            );
            current.pop();
        }
    }

    let mut out = Vec::new();
    if include_bias {
        out.push(Vec::new()); // bias term
    }
    for d in 1..=degree {
        recurse(
            0,
            n_features,
            0,
            d,
            interaction_only,
            &mut Vec::new(),
            &mut out,
        );
    }
    out
}

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

// ═══════════════════════════════════════════════════════════════════════
// Shared helpers
// ═══════════════════════════════════════════════════════════════════════

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml".into(),
        msg: "no input port connected".into(),
    })?;
    input
        .data
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml".into(),
            msg: format!("collect failed: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

fn replace_columns_with_names(
    batches: &[RecordBatch],
    new_col_names: &[String],
    new_data: &Mat<f64>,
) -> Result<RecordBatch, DagError> {
    let schema = batches.first().ok_or(DagError::NodeError {
        node_type: "ml".into(),
        msg: "no input rows".into(),
    })?;
    let orig_schema = schema.schema();
    let mut fields: Vec<Arc<Field>> = orig_schema.fields().iter().cloned().collect();
    let mut arrays: Vec<Arc<dyn Array>> = (0..orig_schema.fields().len())
        .map(|i| batches.first().unwrap().column(i).clone())
        .collect();

    let (nrows, ncols) = new_data.shape();
    for j in 0..ncols {
        let col_data: Vec<f64> = (0..nrows).map(|i| new_data[(i, j)]).collect();
        fields.push(Arc::new(Field::new(
            &new_col_names[j],
            DataType::Float64,
            true,
        )));
        arrays.push(Arc::new(Float64Array::from(col_data)));
    }

    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml".into(),
        msg: format!("failed to build output: {e}"),
    })
}
