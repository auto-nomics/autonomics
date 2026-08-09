//! Writing/LaTeX DAG node bundle.
//!
//! Provides nodes that bridge analysis results (DataFrames) to the LaTeX
//! writing system:
//!
//! - `table_from_df` — converts a DataFrame to a LaTeX tabular string.
//! - `figure_embed` — wraps an image file path as a LaTeX figure block JSON.
//! - `latex_table` — assembles a complete LaTeX table float from a DataFrame.

pub mod figure_node;
pub mod table_node;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;

impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "writing"
    }

    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(table_node::TableFromDfFactory {}));
        registry.register(Box::new(figure_node::FigureEmbedFactory {}));
    }
}
