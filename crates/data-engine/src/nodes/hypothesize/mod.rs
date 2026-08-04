//! `hypothesize.*` DAG nodes — composable hypothesis testing in the DAG engine.
//!
//! Each primitive test node emits a single-row "standard test row" (design
//! doc §2.2) so combinators can chain downstream. See
//! `docs/hypothesize-design.md` for the full catalogue.

pub mod combine_nodes;
pub mod common;
pub mod gof_nodes;
pub mod parametric;
pub mod ranks;
pub mod trinity;
pub mod variance;

/// Register all `hypothesize.*` node factories (24 nodes total).
pub fn register_all(registry: &mut crate::node_registry::registry::NodeRegistry) {
    // Group A — Parametric sample-data tests (5)
    registry.register(Box::new(parametric::TTestNodeFactory));
    registry.register(Box::new(parametric::ZTestNodeFactory));
    registry.register(Box::new(parametric::PropTestNodeFactory));
    registry.register(Box::new(parametric::VarTestNodeFactory));
    registry.register(Box::new(parametric::CorTestNodeFactory));

    // Group A — Nonparametric rank tests (3)
    registry.register(Box::new(ranks::WilcoxonNodeFactory));
    registry.register(Box::new(ranks::KruskalWallisNodeFactory));
    registry.register(Box::new(ranks::FriedmanNodeFactory));

    // Group A — Variance homogeneity + ANOVA (4)
    registry.register(Box::new(variance::BartlettNodeFactory));
    registry.register(Box::new(variance::LeveneNodeFactory));
    registry.register(Box::new(variance::FlignerNodeFactory));
    registry.register(Box::new(variance::AnovaNodeFactory));

    // Group A — Goodness-of-fit + contingency (5)
    registry.register(Box::new(gof_nodes::ChisqGofNodeFactory));
    registry.register(Box::new(gof_nodes::KsTestNodeFactory));
    registry.register(Box::new(gof_nodes::ShapiroNodeFactory));
    registry.register(Box::new(gof_nodes::AdTestNodeFactory));
    registry.register(Box::new(gof_nodes::FisherExactNodeFactory));

    // Group B — Likelihood trinity (3) + invert (1)
    registry.register(Box::new(trinity::WaldNodeFactory));
    registry.register(Box::new(trinity::LrtNodeFactory));
    registry.register(Box::new(trinity::ScoreNodeFactory));
    registry.register(Box::new(trinity::InvertNodeFactory));

    // Group C — Combinators (3)
    registry.register(Box::new(combine_nodes::CombineNodeFactory));
    registry.register(Box::new(combine_nodes::BooleanNodeFactory));
    registry.register(Box::new(combine_nodes::AdjustNodeFactory));
}
