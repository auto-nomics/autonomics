//! Golden test for the donor-level composition Wald statistics, against an
//! independently generated R reference (`stats::glm` binomial).
//!
//! Bugs (node audit 2026-10-04, P0 #4, plus one found by this golden test):
//! (1) the slope variance read `(H⁻¹)₀₀` (`h[1][1]/det` = Var(intercept))
//! instead of `(H⁻¹)₁₁` (`h[0][0]/det`), understating the slope SE by ~26%
//! on typical data and inflating z; (2) the linear predictor was passed
//! through `.exp().sigmoid()` — fitting σ(e^η), not σ(η) — so the reported
//! `log_odds_ratio` was not a log-odds ratio at all (this fixture: −4.777
//! instead of −1.099). The reference below was produced by R 4.6 base
//! `glm` **with `glm.control(epsilon=1e-14)`** — at the default
//! `epsilon=1e-8` R stops ~2e-10 short of the MLE and its SE is off by
//! 1.2e-6, too loose to anchor a 1e-10 golden — and cross-checked against
//! the closed form (balanced two-group fixture: slope = −log 3 exactly,
//! SE = sqrt(52.5/675) from XᵀWX at the MLE):
//!
//! ```r
//! donors <- data.frame(x = c(0,0,0,1,1,1),
//!                      k = c(30,28,32,20,18,22),
//!                      m = c(10,12,8,20,22,18))
//! fit <- glm(cbind(k, m) ~ x, family = binomial, data = donors,
//!            control = glm.control(epsilon = 1e-14))
//! ```
//!
//! T_cell:   slope=-1.0986122886681093 (analytic: -log 3 = -1.0986122886681098)
//!           se=0.2788866755028403    (analytic: sqrt(52.5/675) = 0.2788866755113585)
//!           z=-3.9392785140676994    p=8.172701128924383e-05
//! Myeloid:  same magnitudes, mirrored signs.
//! The pre-fix code returned slope≈−4.777 (wrong model) and se≈0.394
//! (Var(intercept)) for both rows.

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
use arrow_schema::DataType;
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::{DagNode, NodeInput};
use dag_core::registry::{NodeCtx, NodeFactory};
use datafusion::prelude::SessionContext;
use nodes_hypothesize::donor_composition::{DonorCompositionNodeFactory, DONOR_COMPOSITION_KIND};
use std::sync::Arc;

/// Expand donor × cell-type counts into the cell-level table the node
/// consumes: three reference donors at ~75% T_cell, three test donors at 50%.
fn cell_level_batch() -> RecordBatch {
    use arrow_schema::{Field, Schema};
    let counts: &[(&str, &str, &str, usize, usize)] = &[
        // (donor, condition, cell_type, T_cell, Myeloid)
        ("ref1", "reference", "T_cell", 30, 10),
        ("ref2", "reference", "T_cell", 28, 12),
        ("ref3", "reference", "T_cell", 32, 8),
        ("test1", "test", "T_cell", 20, 20),
        ("test2", "test", "T_cell", 18, 22),
        ("test3", "test", "T_cell", 22, 18),
    ];
    let mut donors = Vec::new();
    let mut conditions = Vec::new();
    let mut cell_types = Vec::new();
    for &(donor, condition, _, t_cell, myeloid) in counts {
        for _ in 0..t_cell {
            donors.push(donor);
            conditions.push(condition);
            cell_types.push("T_cell");
        }
        for _ in 0..myeloid {
            donors.push(donor);
            conditions.push(condition);
            cell_types.push("Myeloid");
        }
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("donor_id", DataType::Utf8, false),
        Field::new("condition", DataType::Utf8, false),
        Field::new("cell_type", DataType::Utf8, false),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(donors)),
            Arc::new(StringArray::from(conditions)),
            Arc::new(StringArray::from(cell_types)),
        ],
    )
    .unwrap()
}

#[tokio::test]
async fn donor_composition_matches_r_glm_reference() {
    let session = SessionContext::new();
    let spec = serde_json::json!({
        "donor_col": "donor_id",
        "cell_type_col": "cell_type",
        "condition_col": "condition",
        "reference": "reference",
        "test": "test",
        "correction": "BH",
    });
    let node = DonorCompositionNodeFactory
        .build(spec, NodeCtx::new(session.runtime_env(), None))
        .unwrap();
    assert_eq!(node.kind(), DONOR_COMPOSITION_KIND);
    let mut node = node;
    let df = session.read_batch(cell_level_batch()).unwrap();
    let outputs = node
        .execute(
            &NodeCtx::new(session.runtime_env(), None),
            &[NodeInput::new_dataframe(0, df)],
            &NodeReporter::noop(),
        )
        .await
        .unwrap();

    let batches = outputs.dataframe(0).unwrap().clone().collect().await.unwrap();
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 2, "one row per cell type");

    let cell_types = batch
        .column_by_name("cell_type")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let f = |name: &str| -> Vec<f64> {
        batch
            .column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect()
    };
    let log_or = f("log_odds_ratio");
    let se = f("standard_error");
    let z = f("z");
    let p_raw = f("p_raw");
    let p_adj = f("p_adj");

    // R glm (epsilon=1e-14) reference values, cross-checked against the
    // closed form (see module docs). Ties: BH leaves both adjusted values
    // equal to the raw p. Rows are located by cell-type name — the node
    // emits them in BTreeMap order ("Myeloid" first).
    let slope_ref = -(3.0f64).ln(); // -1.0986122886681098
    let se_ref = (52.5f64 / 675.0f64).sqrt(); // 0.2788866755113585
    let z_ref = -3.939_278_514_067_699_4; // R, epsilon=1e-14
    let p_ref = 8.172_701_128_924_383e-5; // R, epsilon=1e-14
    for name in ["T_cell", "Myeloid"] {
        let row = (0..batch.num_rows())
            .find(|&i| cell_types.value(i) == name)
            .unwrap_or_else(|| panic!("row for {name} missing"));
        let sign = if name == "T_cell" { 1.0 } else { -1.0 };
        assert!(
            (log_or[row] - sign * slope_ref).abs() < 1e-12,
            "{name} slope: got {}, want {}",
            log_or[row],
            sign * slope_ref
        );
        // The audit bug returned sqrt(Var(intercept)) ≈ 0.394 here; the
        // exp-before-sigmoid bug returned a slope of ≈ −4.777.
        assert!(
            (se[row] - se_ref).abs() < 1e-10,
            "{name} SE: got {}, want {se_ref} (sqrt(52.5/675))",
            se[row]
        );
        assert!(
            (z[row] - sign * z_ref).abs() < 1e-9,
            "{name} z: got {}, want {}",
            z[row],
            sign * z_ref
        );
        assert!(
            ((p_raw[row] - p_ref) / p_ref).abs() < 1e-9,
            "{name} p_raw: got {}, want {p_ref}",
            p_raw[row]
        );
        assert!(
            ((p_adj[row] - p_ref) / p_ref).abs() < 1e-9,
            "{name} p_adj (BH, tied): got {}, want {p_ref}",
            p_adj[row]
        );
    }
}
