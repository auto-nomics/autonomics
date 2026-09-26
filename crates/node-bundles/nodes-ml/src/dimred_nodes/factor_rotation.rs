use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Factor rotation (varimax / promax)
// ═══════════════════════════════════════════════════════════════════════

fn default_method() -> String {
    "varimax".to_string()
}
fn default_normalize() -> bool {
    true
}
fn default_eps() -> f64 {
    1e-5
}
fn default_power() -> f64 {
    4.0
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FactorRotationSpec {
    /// Loading-matrix column names — one column per factor (k ≥ 2).
    pub loadings: Vec<String>,
    /// Optional label column naming each row (variable names); falls back to
    /// `row_{i}`.
    #[serde(default)]
    pub labels_column: Option<String>,
    /// `"varimax"` (orthogonal, default) or `"promax"` (oblique).
    #[serde(default = "default_method")]
    pub method: String,
    /// Kaiser row normalisation (varimax only; R default true — promax
    /// hard-codes it).
    #[serde(default = "default_normalize")]
    pub normalize: bool,
    /// Convergence threshold on the relative criterion change (R default 1e-5).
    #[serde(default = "default_eps")]
    pub eps: f64,
    /// Promax power (R default 4).
    #[serde(default = "default_power")]
    pub m: f64,
}

pub struct FactorRotationFactory;
impl NodeFactory for FactorRotationFactory {
    fn kind(&self) -> &'static str {
        "ml_factor_rotation"
    }
    fn desc(&self) -> &'static str {
        "Rotate a factor loading matrix (varimax / promax)."
    }
    fn doc(&self) -> &'static str {
        "Faithful port of R stats::varimax / stats::promax. Input: a loading \
         matrix as k named columns (rows = variables). Output 0: the rotated \
         loadings (label + factor_0..factor_{k-1}); output 1: the rotation \
         matrix (k × k, orthogonal for varimax, oblique for promax). \
         R defaults preserved: varimax(normalize=TRUE, eps=1e-5), \
         promax(m=4)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FactorRotationSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None)
            .add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: FactorRotationSpec = serde_json::from_value(spec)?;
        let reject = |reason: String| dag_core::registry::error::Error::SpecRejection {
            kind: "ml_factor_rotation".to_string(),
            reason,
            schema_pretty: serde_json::to_string_pretty(&self.spec_schema()).unwrap_or_default(),
        };
        if s.loadings.len() < 2 {
            return Err(reject(
                "loadings must name ≥ 2 columns (rotation needs ≥ 2 factors)".into(),
            ));
        }
        match s.method.as_str() {
            "varimax" | "promax" => {}
            other => {
                return Err(reject(format!(
                    "unknown method='{other}'. Supported: 'varimax', 'promax'."
                )));
            }
        }
        Ok(Box::new(FactorRotationNode {
            loadings: s.loadings,
            labels_column: s.labels_column,
            method: s.method,
            normalize: s.normalize,
            eps: s.eps,
            m: s.m,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct FactorRotationNode {
    loadings: Vec<String>,
    labels_column: Option<String>,
    method: String,
    normalize: bool,
    eps: f64,
    m: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for FactorRotationNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_factor_rotation"
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
        let ne = |msg: String| DagError::NodeError {
            node_type: "ml_factor_rotation".into(),
            msg,
        };
        let batches = collect_batches(inputs).await?;
        let data =
            common::extract_matrix(&batches, &self.loadings).map_err(|e| ne(e.to_string()))?;
        let (rows, cols) = data.shape();
        if rows < 2 {
            return Err(ne("loading matrix needs ≥ 2 rows (variables)".into()));
        }

        let rotated = match self.method.as_str() {
            "varimax" => ml::rotation::varimax(&data, self.normalize, self.eps),
            _ => ml::rotation::promax(&data, self.m),
        }
        .map_err(|e| ne(e.to_string()))?;

        // Row labels: from the optional string column, else row_{i}.
        let labels: Vec<String> = match &self.labels_column {
            Some(col) => {
                common::extract_string_column(&batches, col).map_err(|e| ne(e.to_string()))?
            }
            None => (0..rows).map(|i| format!("row_{i}")).collect(),
        };
        if labels.len() != rows {
            return Err(ne(format!(
                "labels column has {} rows but the loading matrix has {rows}",
                labels.len()
            )));
        }

        // Output 0: rotated loadings — label + factor_0..factor_{k-1}.
        let mut fields = vec![Field::new("label", DataType::Utf8, false)];
        for j in 0..cols {
            fields.push(Field::new(format!("factor_{j}"), DataType::Float64, false));
        }
        let mut arrays: Vec<Arc<dyn Array>> =
            vec![Arc::new(arrow_array::StringArray::from(labels))];
        for j in 0..cols {
            arrays.push(Arc::new(Float64Array::from(
                (0..rows)
                    .map(|i| rotated.loadings[(i, j)])
                    .collect::<Vec<f64>>(),
            )));
        }
        let loadings_batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| ne(format!("loadings batch: {e}")))?;

        // Output 1: rotation matrix — k × k, rows factor_in, cols factor_out.
        let mut rfields = vec![Field::new("factor_in", DataType::Utf8, false)];
        for j in 0..cols {
            rfields.push(Field::new(format!("factor_{j}"), DataType::Float64, false));
        }
        let row_names: Vec<String> = (0..cols).map(|i| format!("factor_{i}")).collect();
        let mut rarrays: Vec<Arc<dyn Array>> =
            vec![Arc::new(arrow_array::StringArray::from(row_names))];
        for j in 0..cols {
            rarrays.push(Arc::new(Float64Array::from(
                (0..cols)
                    .map(|i| rotated.rotmat[(i, j)])
                    .collect::<Vec<f64>>(),
            )));
        }
        let rotmat_batch = RecordBatch::try_new(Arc::new(Schema::new(rfields)), rarrays)
            .map_err(|e| ne(format!("rotmat batch: {e}")))?;

        let session = ctx.session();
        let df_loadings = session
            .read_batch(loadings_batch)
            .map_err(|e| ne(format!("read_batch: {e}")))?;
        let df_rotmat = session
            .read_batch(rotmat_batch)
            .map_err(|e| ne(format!("read_batch: {e}")))?;
        let mut res = PortOutputs::new();
        res.insert(0, df_loadings);
        res.insert(1, df_rotmat);
        Ok(res)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    /// 8×3 loading matrix with two clean factors + a diffuse third.
    fn make_batch() -> RecordBatch {
        let labels: Vec<&str> = vec!["v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8"];
        let f1 = [0.85, 0.82, 0.78, 0.10, 0.12, 0.09, 0.55, 0.44];
        let f2 = [0.11, 0.14, 0.18, 0.80, 0.76, 0.72, 0.42, 0.50];
        let f3 = [0.12, 0.30, 0.25, 0.15, 0.30, 0.20, 0.28, 0.33];
        let to_arr = |v: Vec<f64>| Arc::new(Float64Array::from(v)) as Arc<dyn Array>;
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("label", DataType::Utf8, false),
                Field::new("f1", DataType::Float64, false),
                Field::new("f2", DataType::Float64, false),
                Field::new("f3", DataType::Float64, false),
            ])),
            vec![
                Arc::new(arrow_array::StringArray::from(labels)),
                to_arr(f1.to_vec()),
                to_arr(f2.to_vec()),
                to_arr(f3.to_vec()),
            ],
        )
        .unwrap()
    }

    async fn run(spec: serde_json::Value) -> (Vec<RecordBatch>, Vec<RecordBatch>) {
        let mut node = FactorRotationFactory.build(spec, node_ctx()).unwrap();
        let input = dag_core::node::NodeInput::new_dataframe(
            0,
            datafusion::prelude::SessionContext::new()
                .read_batch(make_batch())
                .unwrap(),
        );
        let outs = node
            .execute(
                &node_ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let loadings = outs.dataframe(0).unwrap().clone().collect().await.unwrap();
        let rotmat = outs.dataframe(1).unwrap().clone().collect().await.unwrap();
        (loadings, rotmat)
    }

    fn f(rows: &[RecordBatch], row: usize, col: &str) -> f64 {
        let batch = &rows[0];
        let idx = batch.schema().index_of(col).unwrap();
        batch
            .column(idx)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(row)
    }

    #[tokio::test]
    async fn varimax_end_to_end() {
        let (loadings, rotmat) = run(serde_json::json!({
            "loadings": ["f1", "f2", "f3"],
            "labels_column": "label",
        }))
        .await;
        assert_eq!(loadings.iter().map(|b| b.num_rows()).sum::<usize>(), 8);
        assert_eq!(rotmat.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
        // Labels threaded through.
        let lab = loadings[0]
            .column(loadings[0].schema().index_of("label").unwrap())
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(lab.value(0), "v1");
        // Total sums of squares preserved (orthogonal rotation).
        let ssq = |rows: &[RecordBatch]| -> f64 {
            (0..8)
                .map(|i| {
                    (0..3)
                        .map(|j| f(rows, i, &format!("factor_{j}")).powi(2))
                        .sum::<f64>()
                })
                .sum()
        };
        // (input ssq computed from the fixture constants)
        let input_ssq: f64 = [0.85, 0.82, 0.78, 0.10, 0.12, 0.09, 0.55, 0.44]
            .iter()
            .map(|v| v * v)
            .sum::<f64>()
            + [0.11, 0.14, 0.18, 0.80, 0.76, 0.72, 0.42, 0.50]
                .iter()
                .map(|v| v * v)
                .sum::<f64>()
            + [0.12, 0.30, 0.25, 0.15, 0.30, 0.20, 0.28, 0.33]
                .iter()
                .map(|v| v * v)
                .sum::<f64>();
        assert!((ssq(&loadings) - input_ssq).abs() < 1e-9);
        // rotmat orthonormal.
        for i in 0..3 {
            for j in 0..3 {
                let dot: f64 = (0..3)
                    .map(|t| {
                        f(&rotmat, t, &format!("factor_{i}"))
                            * f(&rotmat, t, &format!("factor_{j}"))
                    })
                    .sum();
                let expect = if i == j { 1.0 } else { 0.0 };
                assert!((dot - expect).abs() < 1e-9, "rotmat ({i},{j}) dot {dot}");
            }
        }
    }

    #[tokio::test]
    async fn promax_end_to_end() {
        let (loadings, rotmat) = run(serde_json::json!({
            "loadings": ["f1", "f2", "f3"],
            "method": "promax",
            "m": 4.0,
        }))
        .await;
        // Default labels when no column given.
        let lab = loadings[0]
            .column(loadings[0].schema().index_of("label").unwrap())
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(lab.value(7), "row_7");
        // Oblique rotmat: columns no longer orthonormal, but well-defined.
        assert!(f(&rotmat, 0, "factor_0").is_finite());
        // Structure simplified: v1..v3 load high on one factor after promax.
        let row0: Vec<f64> = (0..3)
            .map(|j| f(&loadings, 0, &format!("factor_{j}")).abs())
            .collect();
        let max0 = row0.iter().cloned().fold(0.0_f64, f64::max);
        assert!(max0 > 0.5, "v1 should load strongly somewhere: {row0:?}");
    }

    #[tokio::test]
    async fn bad_method_and_single_factor_rejected_at_build() {
        let err = FactorRotationFactory.build(
            serde_json::json!({"loadings": ["f1", "f2"], "method": "quartimax"}),
            node_ctx(),
        );
        assert!(err.is_err());
        let err = FactorRotationFactory.build(serde_json::json!({"loadings": ["f1"]}), node_ctx());
        assert!(err.is_err());
    }
}
