//! `evidence_merge`: merge any number of evidence files into one
//! deduplicated set.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use bib_types::evidence::{EvidenceSet, FORMAT};
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, DagNode};
use dag_core::node::{NodeInput, NodePorts};

use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::nodes::{read_file_bytes, write_artifact};

/// Spec for [`EvidenceMergeNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct EvidenceMergeSpec {
    /// Destination for the merged evidence JSON artifact. Prefers a
    /// `vfs://` URI for cross-process artifacts; a bare absolute path is
    /// auto-rewritten to `vfs:///...` so the engine and the agent share
    /// one mounted object-store namespace. Overwritten on re-run. Give
    /// each merge node its own path.
    pub path: String,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label_and_format(None, PortType::File, "evidence", FORMAT)
        .add_output_port_of_type_with_label_and_format(None, PortType::File, "evidence", FORMAT)
        // Variadic: wire as many evidence producers as needed. Declared
        // port 0 stays required — a merge with zero inputs is rejected at
        // wiring time (PortDisconnected).
        .set_fixed_input(false)
}

pub struct EvidenceMergeNode {
    meta: NodePorts,
    spec: EvidenceMergeSpec,
}

impl EvidenceMergeNode {
    pub fn new(spec: EvidenceMergeSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
        }
    }
}

#[async_trait]
impl DagNode for EvidenceMergeNode {
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
        "evidence_merge"
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
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        if inputs.is_empty() {
            return Err(DagError::Schedule(
                "evidence_merge requires at least one wired evidence input".into(),
            ));
        }

        // Edge order from the scheduler is not guaranteed to be port order;
        // sort so the merged record order (and thus the output bytes) is a
        // deterministic function of the wiring.
        let mut sorted: Vec<&NodeInput> = inputs.iter().collect();
        sorted.sort_by_key(|input| input.port);

        let mut merged = EvidenceSet::default();
        for input in sorted {
            let file = input.file_value()?;
            if let Some(format) = file.format.as_deref()
                && format != FORMAT
            {
                return Err(DagError::Schedule(format!(
                    "evidence_merge input port {} carries format `{format}`, expected `{FORMAT}`",
                    input.port
                )));
            }
            let bytes = read_file_bytes(node_ctx, &file.path).await?;
            let set = EvidenceSet::parse(&bytes).map_err(|error| {
                DagError::Schedule(format!(
                    "evidence_merge input port {} (`{}`): {error}",
                    input.port, file.path
                ))
            })?;
            merged.records.extend(set.records);
        }

        let removed = merged.dedup_in_place();
        if removed > 0 {
            reporter.warn(format!(
                "merged {} input set(s): {removed} duplicate record(s) removed, {} kept",
                inputs.len(),
                merged.records.len()
            ));
        }

        let bytes = merged.to_bytes().map_err(DagError::Schedule)?;
        let file = write_artifact(node_ctx, &self.spec.path, bytes, FORMAT).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

pub struct EvidenceMergeNodeFactory {}

#[async_trait]
impl NodeFactory for EvidenceMergeNodeFactory {
    fn kind(&self) -> &'static str {
        "evidence_merge"
    }

    fn desc(&self) -> &'static str {
        "Merge any number of evidence files into one deduplicated set"
    }

    fn doc(&self) -> &'static str {
        "Consumes one or more `evidence` artifacts (format-contract enforced at \
         wiring time) and writes a single merged, deduplicated `evidence` \
         artifact. Inputs are read in port order (the order edges were wired), \
         so the output is deterministic. Records are the same evidence when \
         any identifier collides (DOI normalized and lowercased, then PMID, \
         arXiv, …; records with no identifiers fall back to the fuzzy cite \
         key). First occurrence wins for citation fields and `origin`; a \
         missing `note` is filled from the first duplicate that has one. The \
         same paper arriving with disjoint identifiers (PMID-only vs \
         DOI-only) is kept twice — resolving that needs a cross-source \
         lookup this node intentionally does not perform.\n\
         \n\
         At least one input must be wired. Use it to combine evidence from \
         several source_literature searches (or a diamond wiring) before \
         evidence_export."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EvidenceMergeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: EvidenceMergeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EvidenceMergeNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bib_types::evidence::EvidenceRecord;
    use bib_types::types::{Article, Identifier};
    use dag_core::dag::node_event::NodeReporter;
    use dag_core::node::NodeInput;
    use dag_core::value::FileRef;

    fn node_ctx() -> (NodeCtx, Arc<vfs::OpendalFileStorage>) {
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage.clone()),
        );
        (ctx, storage)
    }

    fn record(id: &str, title: &str, doi: Option<&str>, note: Option<&str>) -> EvidenceRecord {
        let mut citation = Article::new(id, title);
        if let Some(doi) = doi {
            citation.identifiers.push(Identifier::doi(doi));
        }
        EvidenceRecord {
            citation,
            note: note.map(Into::into),
            origin: Some("test".into()),
        }
    }

    async fn write_set(
        storage: &vfs::OpendalFileStorage,
        tag: &str,
        records: Vec<EvidenceRecord>,
    ) -> (String, FileRef) {
        // Absolute virtual path; `read_file_bytes` resolves bare absolute
        // paths through the mounted opendal storage.
        let path = format!("/tmp/merge-node-{tag}-{}.json", uuid::Uuid::new_v4());
        let set = EvidenceSet {
            records,
            ..Default::default()
        };
        let bytes = set.to_bytes().unwrap();
        storage.op.write(&path, bytes).await.unwrap();
        let file = FileRef::new(&path, Some(FORMAT.into()));
        (path, file)
    }

    #[tokio::test]
    async fn merges_in_port_order_and_dedups() {
        let (ctx, storage) = node_ctx();
        let (_path_a, file_a) = write_set(
            &storage,
            "a",
            vec![
                record("1", "Alpha", Some("10.1/a"), None),
                record("2", "Beta", Some("10.1/b"), None),
            ],
        )
        .await;
        let (_path_b, file_b) = write_set(
            &storage,
            "b",
            vec![
                // Same DOI as Alpha: duplicate.
                record(
                    "3",
                    "Alpha (alt view)",
                    Some("https://doi.org/10.1/A"),
                    Some("why"),
                ),
                // No identifiers, distinct cite key from Beta: kept.
                record("4", "Gamma", None, None),
            ],
        )
        .await;

        let out_path = format!("/tmp/merge-node-out-{}.json", uuid::Uuid::new_v4());
        let mut node = EvidenceMergeNode::new(EvidenceMergeSpec {
            path: out_path.clone(),
        });
        let inputs = vec![
            NodeInput::file(1, file_b.clone()),
            NodeInput::file(0, file_a.clone()),
        ];
        let outputs = node
            .execute(&ctx, &inputs, &NodeReporter::noop())
            .await
            .unwrap();

        let merged = EvidenceSet::parse(&read_file_bytes(&ctx, &out_path).await.unwrap()).unwrap();
        // Port order: A's records first, then B's non-duplicate.
        assert_eq!(merged.records.len(), 3);
        assert_eq!(merged.records[0].citation.title, "Alpha");
        assert_eq!(merged.records[1].citation.title, "Beta");
        assert_eq!(merged.records[2].citation.title, "Gamma");
        // Note filled from the duplicate.
        assert_eq!(merged.records[0].note.as_deref(), Some("why"));

        let out_file = outputs
            .get(&0)
            .and_then(|value| value.as_file().ok())
            .unwrap();
        let fingerprint = out_file.fingerprint.as_ref().expect("fingerprint");
        assert!(
            fingerprint
                .content_hash
                .as_deref()
                .is_some_and(|h| h.starts_with("sha256:"))
        );
        assert!(fingerprint.immutable_remote);
        assert!(out_file.path.starts_with("vfs://"), "{}", out_file.path);
    }

    #[tokio::test]
    async fn rejects_non_evidence_format_input() {
        let (ctx, storage) = node_ctx();
        // Inputs read from VFS via the mounted storage; a wrong-format
        // input must trip the format check before any read happens.
        let path = format!("/tmp/merge-node-plain-{}.csv", uuid::Uuid::new_v4());
        storage
            .op
            .write(&path, b"a,b\n1,2\n".to_vec())
            .await
            .unwrap();
        let plain = FileRef::new(&path, Some("csv".into()));
        let mut node = EvidenceMergeNode::new(EvidenceMergeSpec {
            path: "/tmp/should-not-be-written.json".into(),
        });
        let inputs = vec![NodeInput::file(0, plain)];
        let err = node
            .execute(&ctx, &inputs, &NodeReporter::noop())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected `evidence`"), "{err}");
    }

    #[tokio::test]
    async fn rejects_newer_schema_version_payload() {
        let (ctx, storage) = node_ctx();
        let path = format!("/tmp/merge-node-v2-{}.json", uuid::Uuid::new_v4());
        storage
            .op
            .write(&path, br#"{"schema_version": 2, "records": []}"#.to_vec())
            .await
            .unwrap();
        let file = FileRef::new(&path, Some(FORMAT.into()));
        let mut node = EvidenceMergeNode::new(EvidenceMergeSpec {
            path: "/tmp/should-not-be-written.json".into(),
        });
        let inputs = vec![NodeInput::file(0, file)];
        let err = node
            .execute(&ctx, &inputs, &NodeReporter::noop())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("schema_version 2"), "{err}");
    }
}
