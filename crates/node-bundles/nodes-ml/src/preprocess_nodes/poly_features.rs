use super::*;

// ═══════════════════════════════════════════════════════════════════════
// PolyFeatures
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PolyFeaturesSpec {
    #[schemars(length(min = 1))]
    pub columns: Vec<String>,
    #[schemars(range(min = 1), extend("x-strict-type" = true))]
    #[serde(default = "default_degree")]
    pub degree: usize,
    #[serde(default = "default_true")]
    pub include_bias: bool,
    #[serde(default = "default_false")]
    pub interaction_only: bool,
}
fn default_degree() -> usize {
    2
}
fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
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
        "PolynomialFeatures: preserves the selected original columns and appends polynomial combinations up to the specified degree. First-degree terms are represented by the original columns; degree must be at least 1. Defaults follow sklearn (include_bias=true, interaction_only=false)."
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
        if s.degree == 0 {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "ml_poly_features".into(),
                reason: "degree must be at least 1".into(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(PolyFeaturesSpec))
                    .unwrap_or_default(),
            });
        }
        if s.columns.is_empty() {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "ml_poly_features".into(),
                reason: "columns must contain at least one column name".into(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(PolyFeaturesSpec))
                    .unwrap_or_default(),
            });
        }
        let mut unique_columns = std::collections::HashSet::new();
        if !s.columns.iter().all(|column| unique_columns.insert(column)) {
            return Err(dag_core::registry::error::Error::SpecRejection {
                kind: "ml_poly_features".into(),
                reason: "columns must not contain duplicate names".into(),
                schema_pretty: serde_json::to_string_pretty(&schema_for!(PolyFeaturesSpec))
                    .unwrap_or_default(),
            });
        }
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
        let data =
            common::extract_matrix(&batches, &self.columns).map_err(|e| DagError::NodeError {
                node_type: "ml_poly_features".into(),
                msg: e.to_string(),
            })?;
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
        let names = polynomial_feature_names(&self.columns, &combos)?;

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
    // First-degree terms are already present as the selected input columns.
    // Emitting them again would duplicate those fields in the output schema.
    for d in 2..=degree {
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

fn polynomial_feature_names(
    columns: &[String],
    combinations: &[Vec<usize>],
) -> Result<Vec<String>, DagError> {
    combinations
        .iter()
        .map(|combination| {
            if combination.is_empty() {
                return Ok("bias".to_string());
            }

            let mut terms: Vec<(&str, usize)> = Vec::new();
            for &column_index in combination {
                let Some(column) = columns.get(column_index) else {
                    return Err(DagError::NodeError {
                        node_type: "ml_poly_features".into(),
                        msg: format!("invalid feature index {column_index}"),
                    });
                };
                match terms.last_mut() {
                    Some((name, count)) if name == &column.as_str() => *count += 1,
                    _ => terms.push((column.as_str(), 1)),
                }
            }

            Ok(terms
                .into_iter()
                .map(|(name, count)| {
                    if count == 1 {
                        name.to_string()
                    } else {
                        format!("{name}^{count}")
                    }
                })
                .collect::<Vec<_>>()
                .join("*"))
        })
        .collect()
}

#[cfg(test)]
mod poly_tests {
    use super::*;
    use dag_core::registry::NodeRegistry;
    use dag_core::registry::error::Error;
    use datafusion::prelude::SessionContext;

    fn input_batch() -> RecordBatch {
        let schema = Schema::new(vec![
            Field::new("x1", DataType::Float64, false),
            Field::new("x2", DataType::Float64, false),
            Field::new("cat", DataType::Utf8, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(Float64Array::from(vec![1.0, 2.0])),
                Arc::new(Float64Array::from(vec![2.0, 3.0])),
                Arc::new(StringArray::from(vec!["a", "b"])),
            ],
        )
        .unwrap()
    }

    fn poly_registry() -> NodeRegistry {
        let mut registry =
            NodeRegistry::new(NodeCtx::new(SessionContext::new().runtime_env(), None));
        registry.register(Box::new(PolyFeaturesFactory));
        registry
    }

    #[test]
    fn poly_schema_restricts_inputs_and_follows_sklearn_defaults() {
        let schema = serde_json::to_value(schema_for!(PolyFeaturesSpec)).unwrap();
        let columns = &schema["properties"]["columns"];
        let degree = &schema["properties"]["degree"];
        let interaction_only = &schema["properties"]["interaction_only"];

        assert_eq!(columns["minItems"], serde_json::json!(1));
        assert_eq!(degree["minimum"], serde_json::json!(1));
        assert_eq!(degree["x-strict-type"], serde_json::json!(true));
        assert_eq!(interaction_only["default"], serde_json::json!(false));
    }

    #[test]
    fn poly_factory_rejects_invalid_specs() {
        let registry = poly_registry();
        let string_degree = registry
            .build_node(
                "ml_poly_features",
                serde_json::json!({
                    "columns": ["x1"],
                    "degree": "2",
                    "include_bias": false
                }),
            )
            .err()
            .expect("string degree should be rejected");
        assert!(matches!(string_degree, Error::SpecRejection { .. }));
        assert!(string_degree.to_string().contains("got a string"));

        let zero_degree = registry
            .build_node(
                "ml_poly_features",
                serde_json::json!({
                    "columns": ["x1"],
                    "degree": 0
                }),
            )
            .err()
            .expect("zero degree should be rejected");
        assert!(
            zero_degree
                .to_string()
                .contains("degree must be at least 1")
        );

        let empty_columns = registry
            .build_node(
                "ml_poly_features",
                serde_json::json!({
                    "columns": [],
                    "degree": 2
                }),
            )
            .err()
            .expect("empty columns should be rejected");
        assert!(
            empty_columns
                .to_string()
                .contains("columns must contain at least one column name")
        );
    }

    #[test]
    fn first_degree_poly_terms_reuse_original_columns() {
        assert!(polynomial_combinations(2, 1, false, false).is_empty());
        assert_eq!(
            polynomial_combinations(2, 1, true, false),
            vec![Vec::<usize>::new()]
        );
    }

    #[test]
    fn poly_output_appends_only_higher_order_features() {
        let combos = polynomial_combinations(2, 2, true, false);
        assert_eq!(combos, vec![vec![], vec![0, 0], vec![0, 1], vec![1, 1]]);

        let columns = vec!["x1".to_string(), "x2".to_string()];
        let names = polynomial_feature_names(&columns, &combos).unwrap();
        assert_eq!(names, vec!["bias", "x1^2", "x1*x2", "x2^2"]);

        let data = common::mat_from_row_major(2, 4, &[1.0, 1.0, 2.0, 4.0, 1.0, 4.0, 6.0, 9.0]);
        let output = replace_columns_with_names(&[input_batch()], &names, &data).unwrap();
        let dataframe = poly_registry()
            .ctx()
            .session()
            .read_batch(output.clone())
            .unwrap();
        let names: Vec<String> = output
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "x1".to_string(),
                "x2".to_string(),
                "cat".to_string(),
                "bias".to_string(),
                "x1^2".to_string(),
                "x1*x2".to_string(),
                "x2^2".to_string()
            ]
        );
        let dataframe_names: Vec<String> = dataframe
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().to_string())
            .collect();
        assert_eq!(dataframe_names, names);
    }

    #[test]
    fn poly_output_rejects_generated_name_collisions() {
        let names = vec!["x1".to_string()];
        let data = common::mat_from_row_major(2, 1, &[1.0, 2.0]);
        let error = replace_columns_with_names(&[input_batch()], &names, &data).unwrap_err();

        assert!(error.to_string().contains("generated feature name 'x1'"));
        assert!(error.to_string().contains("ml_poly_features"));
    }
}
