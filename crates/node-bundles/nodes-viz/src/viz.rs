//! Visualization node: consumes an upstream `DataFrame` and renders it to a
//! PNG via R/ggplot2.
//!
//! Mirrors [`crate::sink_file::FileSinkNode`]: a [`VizNode`] has exactly one input and
//! produces no output. It collects the upstream `RecordBatch`es, hands them to
//! the `visualization` crate (Arrow IPC → `Rscript` ggplot2), and writes the
//! PNG **into the engine's opendal-virtualized filesystem** (not the host
//! filesystem), so the artifact lives in the same isolated space as source/
//! sink data. The rendered virtual path is surfaced to the agent via
//! `NodeReport.artifact_path` (see [`dag_core::dag::graph`]).

use std::sync::Arc;

use async_trait::async_trait;
use datafusion::common::HashMap;
use fs::OpendalFileStorage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use dag_core::dag::DagError;
use dag_core::dag::graph::PortOutputs;
use dag_core::node::{DagNode, NodeInput, NodePorts};

/// Errors raised by the visualization node.
#[derive(Debug, Error)]
pub enum VizError {
    #[error("Invalid input: {message}")]
    InvalidInput { message: String },
    #[error("Visualization render failed")]
    Render {
        #[source]
        source: visualization::error::VizError,
    },
    #[error(
        "no opendal filesystem registered with the engine — \
             the visualization node needs one to write the PNG"
    )]
    NoOpendalFs,
    #[error("writing the PNG into the opendal filesystem failed: {0}")]
    OpendalWrite(String),
}

impl VizError {
    pub fn to_dag_error(self) -> DagError {
        let msg = self.to_string();
        match self {
            VizError::InvalidInput { message } => DagError::Schedule(message),
            _ => DagError::NodeError {
                node_type: "visualization".to_string(),
                msg,
            },
        }
    }
}

impl ::dag_core::dag::NodeError for VizError {
    fn node_type(&self) -> &str {
        "visualization"
    }
}

/// Spec for a [`VizNode`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VizNodeSpec {
    /// Virtual path (in the engine's opendal filesystem) the rendered PNG is
    /// written to — e.g. `"/plots/scatter.png"`. Resolved relative to the
    /// opendal root, just like sink/source paths.
    pub output_path: String,
    /// ggplot2 R code that runs with a `data.frame` named `df` already bound
    /// to the input data. Must build a ggplot object and assign it to a
    /// variable named `p`. Example:
    /// `p <- ggplot(df, aes(x = bp, y = pval)) + geom_point()`
    pub r_code: String,
    /// Figure width in inches (default 8).
    #[serde(default)]
    pub width: Option<f64>,
    /// Figure height in inches (default 6).
    #[serde(default)]
    pub height: Option<f64>,
    /// Resolution in DPI (default 150).
    #[serde(default)]
    pub dpi: Option<f64>,
}

pub struct VizNode {
    meta: NodePorts,
    /// Virtual path the PNG is written to (reported as `artifact_path`).
    output_path: String,
    r_code: String,
    width: Option<f64>,
    height: Option<f64>,
    dpi: Option<f64>,
    /// The opendal handle the PNG is uploaded through. Required at execute
    /// time; the factory pulls it out of the [`NodeCtx`].
    opendal: Option<Arc<OpendalFileStorage>>,
}

/// Static port layout for every [`VizNode`]: a single untyped input port and
/// no outputs — identical to [`crate::sink_file::FileSinkNode`].
fn port_layout() -> NodePorts {
    NodePorts::new().add_input_port(None)
}

impl VizNode {
    pub fn new(spec: VizNodeSpec, opendal: Option<Arc<OpendalFileStorage>>) -> Self {
        Self {
            meta: port_layout(),
            output_path: spec.output_path,
            r_code: spec.r_code,
            width: spec.width,
            height: spec.height,
            dpi: spec.dpi,
            opendal,
        }
    }

    /// The virtual path the PNG is written to (used to populate the node
    /// report's `artifact_path`).
    pub fn output_path(&self) -> &str {
        &self.output_path
    }
}

pub struct VizNodeFactory {}

impl dag_core::registry::NodeFactory for VizNodeFactory {
    fn kind(&self) -> &'static str {
        "visualization"
    }

    fn desc(&self) -> &'static str {
        "Renders an upstream DataFrame to a PNG via R/ggplot2 (needs Rscript)."
    }

    fn doc(&self) -> &'static str {
        "A visualization node that consumes an upstream DataFrame, hands it to \
        R's ggplot2 (via the Arrow IPC stream format) and writes a PNG to \
        `output_path`. The `r_code` field runs with a data.frame named `df` \
        bound to the input data and must assign a ggplot object to `p`. \
        Requires `Rscript` on PATH with the `arrow` and `ggplot2` R packages \
        installed. One untyped input port; no output ports. The rendered path \
        is returned in the node report's `artifact_path` field."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schemars::schema_for!(VizNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: dag_core::registry::NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: VizNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(VizNode::new(node_spec, node_ctx.opendal.clone())))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut dag_core::codegen::CodegenCtx,
    ) -> std::result::Result<dag_core::codegen::NodeCodegen, dag_core::codegen::CodegenError> {
        use dag_core::codegen::helpers::*;
        let s = parse_spec::<VizNodeSpec>(spec, "visualization")?;
        let input = input_0(ctx);
        let width = s.width.unwrap_or(7.0);
        let height = s.height.unwrap_or(5.0);
        let dpi = s.dpi.unwrap_or(300.0);
        let code = vec![
            format!("# Visualization via ggplot2"),
            format!("df <- {input}"),
            s.r_code.clone(),
            format!(
                "ggsave(\"{}\", plot = p, width = {}, height = {}, dpi = {})",
                s.output_path, width, height, dpi
            ),
        ];
        Ok(dag_core::codegen::NodeCodegen {
            code,
            output_vars: vec![], // viz has no output port
            extra_packages: vec![],
        })
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["ggplot2".into()]
    }
}

#[async_trait]
impl DagNode for VizNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        let cp = Self {
            meta: self.meta.clone(),
            output_path: self.output_path.clone(),
            r_code: self.r_code.clone(),
            width: self.width,
            height: self.height,
            dpi: self.dpi,
            opendal: self.opendal.clone(),
        };
        Box::new(cp)
    }

    fn kind(&self) -> &'static str {
        "visualization"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn artifact_path(&self) -> Option<&str> {
        Some(&self.output_path)
    }

    async fn execute(
        &mut self,
        _ctx: &dag_core::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let storage = self.opendal.clone().ok_or(VizError::NoOpendalFs)?;

        let input = inputs.first().ok_or(VizError::InvalidInput {
            message: "VizNode requires exactly one upstream input".to_string(),
        })?;

        // Collect the upstream DataFrame to concrete RecordBatches for the
        // renderer. This is the eager materialization point — visualization
        // needs all rows in memory to draw them.
        let batches = input
            .data
            .clone()
            .collect()
            .await
            .map_err(|e| DagError::NodeError {
                node_type: "visualization".to_string(),
                msg: format!("collecting input failed: {e}"),
            })?;

        // Render the PNG to bytes in a private tempdir (R writes to scratch,
        // never to the caller's filesystem), then upload the bytes into the
        // engine's opendal-virtualized filesystem at the requested path.
        let (png_bytes, diag) = visualization::render::render_png_bytes(
            &batches,
            &self.r_code,
            self.width,
            self.height,
            self.dpi,
        )
        .await
        .map_err(|source| VizError::Render { source })?;

        // Surface non-fatal R warnings/messages for debugging. These don't
        // fail the render but are useful diagnostics for the caller.
        if !diag.warnings.is_empty() || !diag.messages.is_empty() {
            tracing::debug!(
                node = "visualization",
                output = %self.output_path,
                warnings = diag.warnings.len(),
                messages = diag.messages.len(),
                "R render completed with non-fatal diagnostics"
            );
        }

        let virtual_path = OpendalFileStorage::normalize_path(&self.output_path);
        storage
            .op
            .write(&virtual_path, png_bytes)
            .await
            .map_err(|e| VizError::OpendalWrite(e.to_string()))?;

        // No DataFrame output — like SinkNode.
        Ok(HashMap::new())
    }
}

#[cfg(test)]
mod tests {
    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            iceberg_catalog: None,
            datalake: std::sync::Arc::new(datalake::Datalake::default()),
            opendal: None,
        }
    }
    use super::*;
    use arrow_array::{Float64Array, Int32Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::prelude::SessionContext;
    use std::sync::Arc;

    /// Build a small in-memory DataFrame for viz tests.
    fn sample_dataframe() -> datafusion::dataframe::DataFrame {
        let ctx = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int32, false),
            Field::new("y", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5])),
                Arc::new(Float64Array::from(vec![1.0, 4.0, 9.0, 16.0, 25.0])),
            ],
        )
        .expect("sample batch");
        ctx.read_batch(batch).expect("ctx reads batch")
    }

    /// The node renders its input to a PNG written **through opendal** (into
    /// the virtualized root), not the host filesystem. We point opendal at a
    /// tempdir, render, then read the PNG back via the operator to confirm it
    /// landed in the virtual space. Requires `Rscript` + arrow/ggplot2 on PATH.
    #[tokio::test]
    async fn test_viz_node_renders_png_via_opendal() {
        let root = tempfile::tempdir().expect("tempdir for opendal root");
        let storage = Arc::new(OpendalFileStorage::new(root.path()));

        let virtual_out = format!("/viz_node_{}.png", std::process::id());
        let mut node = VizNode::new(
            VizNodeSpec {
                output_path: virtual_out.clone(),
                r_code: "p <- ggplot(df, aes(x = x, y = y)) + geom_point() + geom_line()"
                    .to_string(),
                width: Some(6.0),
                height: Some(4.0),
                dpi: Some(100.0),
            },
            Some(storage.clone()),
        );

        let df = sample_dataframe();
        let res = node
            .execute(
                &node_ctx(),
                &[NodeInput { port: 0, data: df }],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .expect("execute should succeed");
        assert!(res.is_empty(), "viz node has no output ports");

        // The opendal Fs backend is rooted at `root`. The render tempdir is a
        // *separate* tempdir (deleted when render_png_bytes returns), so a PNG
        // appearing under `root` proves it was written through opendal — not
        // directly to the host filesystem.
        let host_path = root.path().join(virtual_out.trim_start_matches('/'));
        let bytes = std::fs::read(&host_path).expect("png landed under opendal root");
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..4], &[0x89, b'P', b'N', b'G']);
        assert_eq!(node.output_path(), virtual_out);
        eprintln!(
            "VizNode OK: {} bytes via opendal at {virtual_out}",
            bytes.len()
        );
    }
}
