//! `source_s2_recommendations`: Semantic Scholar paper recommendations for
//! a seed paper, emitted as an `evidence` artifact (the node counterpart of
//! the retired `s2_recommendations` tool).

use std::sync::Arc;

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use bib_types::evidence::{EvidenceRecord, EvidenceSet, FORMAT};
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, DagNode};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::nodes::literature_citations::shared_s2_client;
use crate::nodes::write_artifact;

const DEFAULT_LIMIT: u32 = 10;
const MAX_LIMIT: u32 = 500;

/// Spec for [`S2RecommendationsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct S2RecommendationsSpec {
    /// Seed paper identifier in any Semantic Scholar form: S2 SHA,
    /// `CorpusId:<id>`, `DOI:<doi>`, `ARXIV:<id>`, `PMID:<id>`,
    /// `PMCID:<id>`, or `URL:<url>`.
    pub paper_id: String,
    /// Maximum recommended papers (default 10, hard cap 500).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Destination for the evidence JSON artifact. `vfs://` URI or absolute
    /// local path; overwritten on re-run.
    pub path: String,
    /// Optional provenance note stamped on every record.
    #[serde(default)]
    pub note: Option<String>,
}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port_of_type_with_label_and_format(
        None,
        PortType::File,
        "evidence",
        FORMAT,
    )
}

pub struct S2RecommendationsNode {
    meta: NodePorts,
    spec: S2RecommendationsSpec,
    /// Test seam: injected client.
    client: Option<Arc<semantic_scholar::S2Client>>,
}

impl S2RecommendationsNode {
    pub fn new(spec: S2RecommendationsSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
            client: None,
        }
    }

    /// Build a node bound to a caller-supplied client (tests).
    pub fn with_client(spec: S2RecommendationsSpec, client: Arc<semantic_scholar::S2Client>) -> Self {
        Self {
            meta: port_layout(),
            spec,
            client: Some(client),
        }
    }
}

#[async_trait]
impl DagNode for S2RecommendationsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            meta: self.meta.clone(),
            spec: self.spec.clone(),
            client: self.client.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        "source_s2_recommendations"
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
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        let client = self.client.clone().unwrap_or_else(shared_s2_client);
        let limit = self.spec.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);

        let resp = client
            .recommendations(&self.spec.paper_id, limit, None)
            .await
            .map_err(|error| {
                DagError::Schedule(format!(
                    "recommendations for `{}` failed: {error}",
                    self.spec.paper_id
                ))
            })?;
        if resp.recommended_papers.is_empty() {
            return Err(DagError::Schedule(format!(
                "no recommended papers returned for `{}` — check the paper id",
                self.spec.paper_id
            )));
        }

        let set = EvidenceSet {
            records: resp
                .recommended_papers
                .iter()
                .map(|paper| EvidenceRecord {
                    citation: semantic_scholar::convert::paper_to_article(paper),
                    note: self.spec.note.clone(),
                    origin: Some("s2-recommendations".into()),
                })
                .collect(),
            ..Default::default()
        };
        let bytes = set.to_bytes().map_err(DagError::Schedule)?;
        let file = write_artifact(node_ctx, &self.spec.path, bytes, FORMAT).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

pub struct S2RecommendationsNodeFactory {}

#[async_trait]
impl NodeFactory for S2RecommendationsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_s2_recommendations"
    }

    fn desc(&self) -> &'static str {
        "Get papers recommended as related to a seed paper, as an evidence file"
    }

    fn doc(&self) -> &'static str {
        "Semantic Scholar's recommendation engine for one seed paper: topically \
         related work likely to share the research thread. Writes an \
         `evidence` artifact (File, format `evidence`) with `origin: \
         s2-recommendations`. `paper_id` accepts any S2 form (SHA, \
         `CorpusId:<id>`, `DOI:<doi>`, `ARXIV:<id>`, `PMID:<id>`, `URL:<url>`). \
         Fails when the response is empty. Chain into evidence_merge with \
         search and citation-graph results."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(S2RecommendationsSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: S2RecommendationsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(S2RecommendationsNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_defaults_direction_and_limits() {
        // Spec-level checks: defaults resolve inside execute; here we pin the
        // constants so accidental changes surface in review.
        assert_eq!(DEFAULT_LIMIT, 10);
        assert_eq!(MAX_LIMIT, 500);
    }
}
