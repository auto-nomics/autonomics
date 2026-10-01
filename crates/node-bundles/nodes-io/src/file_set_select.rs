//! Select one concrete File from an upstream FileSet.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileRef, PortType};

pub const FILE_SET_SELECT_KIND: &str = "file_set_select";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileSetSelectSpec {
    /// Exact FileRef path to select.
    #[serde(default)]
    pub path: Option<String>,
    /// Glob matched against each FileRef path.
    #[serde(default)]
    pub glob: Option<String>,
    /// Zero-based index in the upstream FileSet.
    #[serde(default)]
    pub index: Option<usize>,
    /// Require the glob to match exactly one file. The default is true.
    #[serde(default = "default_true")]
    pub require_unique: bool,
}

fn default_true() -> bool {
    true
}

pub struct FileSetSelectNode {
    ports: NodePorts,
    spec: FileSetSelectSpec,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::FileSet, "files")
        .add_output_port_of_type(None, PortType::File)
}

impl FileSetSelectNode {
    pub fn new(spec: FileSetSelectSpec) -> Self {
        Self {
            ports: port_layout(),
            spec,
        }
    }

    fn select(&self, files: &[FileRef]) -> Result<FileRef, DagError> {
        if let Some(index) = self.spec.index {
            return files.get(index).cloned().ok_or_else(|| {
                DagError::Schedule(format!(
                    "file_set_select index {index} is outside a FileSet of {} files",
                    files.len()
                ))
            });
        }

        if let Some(path) = &self.spec.path {
            return files
                .iter()
                .find(|file| &file.path == path)
                .cloned()
                .ok_or_else(|| DagError::Schedule(format!("FileSet has no path `{path}`")));
        }

        if let Some(pattern) = &self.spec.glob {
            let pattern = glob::Pattern::new(pattern).map_err(|error| {
                DagError::Schedule(format!("invalid file_set_select glob `{pattern}`: {error}"))
            })?;
            let matches: Vec<&FileRef> = files
                .iter()
                .filter(|file| pattern.matches(&file.path))
                .collect();
            if matches.len() != 1 {
                if self.spec.require_unique {
                    return Err(DagError::Schedule(format!(
                        "file_set_select glob matched {} files; expected exactly one",
                        matches.len()
                    )));
                }
                return matches.first().map(|file| (*file).clone()).ok_or_else(|| {
                    DagError::Schedule("file_set_select glob matched no files".into())
                });
            }
            return Ok(matches[0].clone());
        }

        Err(DagError::Schedule(
            "file_set_select requires one of path, glob, or index".into(),
        ))
    }
}

#[async_trait]
impl DagNode for FileSetSelectNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            spec: self.spec.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        FILE_SET_SELECT_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        _ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let files = inputs
            .first()
            .ok_or_else(|| {
                DagError::Schedule("file_set_select requires an upstream FileSet".into())
            })?
            .data
            .as_file_set()?;
        let file = self.select(files)?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

pub struct FileSetSelectNodeFactory;

impl NodeFactory for FileSetSelectNodeFactory {
    fn kind(&self) -> &'static str {
        FILE_SET_SELECT_KIND
    }

    fn desc(&self) -> &'static str {
        "Selects one File from an ordered FileSet."
    }

    fn doc(&self) -> &'static str {
        "Selects by exact FileRef path, glob, or zero-based index. Glob selection \
        defaults to fail-closed and requires exactly one match unless \
        require_unique=false. The selected File preserves its format and fingerprint."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(FileSetSelectSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: FileSetSelectSpec = serde_json::from_value(spec)?;
        let selectors = [
            spec.path.is_some(),
            spec.glob.is_some(),
            spec.index.is_some(),
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count();
        if selectors != 1 {
            return Err("exactly one of path, glob, or index is required".into());
        }
        Ok(Box::new(FileSetSelectNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dag_core::value::NodeValue;

    fn file(path: &str, format: &str) -> FileRef {
        FileRef::new(path, Some(format.into()))
    }

    #[tokio::test]
    async fn selects_unique_glob_and_preserves_metadata() {
        let spec = FileSetSelectSpec {
            path: None,
            glob: Some("vfs:///extracted/**/*.csv".into()),
            index: None,
            require_unique: true,
        };
        let mut node = FileSetSelectNode::new(spec);
        let input = NodeInput {
            port: 0,
            data: NodeValue::FileSet(vec![
                file("vfs:///extracted/a/b.txt", "txt"),
                file("vfs:///extracted/a/table.csv", "csv"),
            ]),
        };
        let outputs = node
            .execute(
                &ctx(),
                &[input],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let selected = outputs.get(&0).unwrap().as_file().unwrap();
        assert_eq!(selected.path, "vfs:///extracted/a/table.csv");
        assert_eq!(selected.format.as_deref(), Some("csv"));
    }

    fn ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    #[test]
    fn factory_rejects_multiple_selectors() {
        let error = match FileSetSelectNodeFactory
            .build(serde_json::json!({"path": "/a", "index": 0}), ctx())
        {
            Ok(_) => panic!("multiple selectors must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("exactly one"));
    }
}
