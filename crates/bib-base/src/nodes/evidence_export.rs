//! `evidence_export`: render an evidence file as bibtex / ris / markdown.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use bib_types::evidence::{EvidenceSet, FORMAT};
use bib_types::types::ExportFormat;
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, DagNode};
use dag_core::node::{NodeInput, NodePorts};

use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::export::render_all;
use crate::nodes::{read_file_bytes, write_artifact};

/// Citation format of an exported file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceExportFormat {
    Bibtex,
    Ris,
    Markdown,
}

impl EvidenceExportFormat {
    /// Format label on the exported `FileRef` and the output port contract.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bibtex => "bibtex",
            Self::Ris => "ris",
            Self::Markdown => "markdown",
        }
    }

    fn export_format(self) -> ExportFormat {
        match self {
            Self::Bibtex => ExportFormat::Bibtex,
            Self::Ris => ExportFormat::Ris,
            Self::Markdown => ExportFormat::Markdown,
        }
    }
}

/// Spec for [`EvidenceExportNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EvidenceExportSpec {
    /// Citation format of the exported file.
    pub format: EvidenceExportFormat,
    /// Destination path (`vfs://` URI or absolute local path; overwritten on
    /// re-run).
    pub path: String,
}

fn port_layout(format: EvidenceExportFormat) -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label_and_format(None, PortType::File, "evidence", FORMAT)
        .add_output_port_of_type_with_label_and_format(
            None,
            PortType::File,
            "export",
            format.label(),
        )
}

pub struct EvidenceExportNode {
    meta: NodePorts,
    spec: EvidenceExportSpec,
}

impl EvidenceExportNode {
    pub fn new(spec: EvidenceExportSpec) -> Self {
        Self {
            meta: port_layout(spec.format),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for EvidenceExportNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            meta: self.meta.clone(),
            spec: self.spec.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        "evidence_export"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.spec.path)
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| {
            DagError::Schedule("evidence_export requires exactly one evidence input".into())
        })?;
        let file = input.file_value()?;
        if let Some(format) = file.format.as_deref()
            && format != FORMAT
        {
            return Err(DagError::Schedule(format!(
                "evidence_export input carries format `{format}`, expected `{FORMAT}`"
            )));
        }

        let bytes = read_file_bytes(node_ctx, &file.path).await?;
        let set = EvidenceSet::parse(&bytes)
            .map_err(|error| DagError::Schedule(format!("evidence_export input `{}`: {error}", file.path)))?;
        let articles: Vec<bib_types::Article> =
            set.records.iter().map(|record| record.citation.clone()).collect();
        let rendered = render_all(&articles, self.spec.format.export_format());

        let file = write_artifact(
            node_ctx,
            &self.spec.path,
            rendered.into_bytes(),
            self.spec.format.label(),
        )
        .await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

pub struct EvidenceExportNodeFactory {}

#[async_trait]
impl NodeFactory for EvidenceExportNodeFactory {
    fn kind(&self) -> &'static str {
        "evidence_export"
    }

    fn desc(&self) -> &'static str {
        "Render an evidence file as a bibtex / ris / markdown bibliography"
    }

    fn doc(&self) -> &'static str {
        "Consumes one `evidence` artifact and renders its citations as a \
         bibliography file: `format: bibtex` (.bib), `ris` (.ris), or \
         `markdown` (numbered reference list). Records are exported in \
         evidence order. The output port's format contract follows the spec \
         (`bibtex` / `ris` / `markdown`), so wiring the export into a port \
         expecting a different format fails at add_edge time. Pair with \
         evidence_merge when combining several searches first."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EvidenceExportSpec)
    }

    fn ports(&self) -> NodePorts {
        // Static default layout; the output format contract follows the spec
        // (see ports_for_spec).
        port_layout(EvidenceExportFormat::Bibtex)
    }

    fn ports_for_spec(&self, spec: serde_json::Value) -> dag_core::registry::error::Result<NodePorts> {
        let spec: EvidenceExportSpec = serde_json::from_value(spec)?;
        Ok(port_layout(spec.format))
    }

    fn build(&self, spec: serde_json::Value, _node_ctx: NodeCtx) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EvidenceExportSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EvidenceExportNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bib_types::evidence::EvidenceRecord;
    use bib_types::types::{Article, Identifier};
    use dag_core::dag::node_event::NodeReporter;
    use dag_core::value::FileRef;

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    async fn write_set(tag: &str) -> String {
        let mut citation = Article::new("1", "A breakthrough");
        citation.identifiers.push(Identifier::doi("10.1000/test"));
        let set = EvidenceSet {
            records: vec![EvidenceRecord {
                citation,
                note: None,
                origin: Some("test".into()),
            }],
            ..Default::default()
        };
        let path = std::env::temp_dir()
            .join(format!("export-node-{tag}-{}.json", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        tokio::fs::write(&path, set.to_bytes().unwrap()).await.unwrap();
        path
    }

    async fn export(format: EvidenceExportFormat) -> (String, String) {
        let in_path = write_set("in").await;
        let out_path = std::env::temp_dir()
            .join(format!(
                "export-node-out-{}.{}",
                uuid::Uuid::new_v4(),
                format.label()
            ))
            .to_string_lossy()
            .into_owned();
        let mut node = EvidenceExportNode::new(EvidenceExportSpec {
            format,
            path: out_path.clone(),
        });
        let inputs = vec![NodeInput::file(0, FileRef::local(&in_path, Some(FORMAT.into())).unwrap())];
        let outputs = node
            .execute(&node_ctx(), &inputs, &NodeReporter::noop())
            .await
            .unwrap();
        let out_file = outputs
            .get(&0)
            .and_then(|value| value.as_file().ok())
            .unwrap();
        assert_eq!(out_file.format.as_deref(), Some(format.label()));
        let content = String::from_utf8(tokio::fs::read(&out_path).await.unwrap()).unwrap();
        std::fs::remove_file(in_path).ok();
        (out_path, content)
    }

    #[tokio::test]
    async fn exports_bibtex_ris_markdown() {
        let (path, content) = export(EvidenceExportFormat::Bibtex).await;
        assert!(content.starts_with("@article{"), "{content}");
        std::fs::remove_file(path).ok();

        let (path, content) = export(EvidenceExportFormat::Ris).await;
        assert!(content.starts_with("TY  - JOUR"), "{content}");
        std::fs::remove_file(path).ok();

        let (path, content) = export(EvidenceExportFormat::Markdown).await;
        assert!(content.starts_with("- "), "{content}");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn ports_for_spec_follows_format() {
        let factory = EvidenceExportNodeFactory {};
        let base = factory.ports();
        assert_eq!(base.output_port(0).unwrap().format.as_deref(), Some("bibtex"));
        let spec = serde_json::json!({"format": "ris", "path": "/tmp/out.ris"});
        let ports = factory.ports_for_spec(spec).unwrap();
        assert_eq!(ports.output_port(0).unwrap().format.as_deref(), Some("ris"));
        // Input contract is evidence in both layouts.
        assert_eq!(ports.input_port(0).unwrap().format.as_deref(), Some(FORMAT));
    }
}
