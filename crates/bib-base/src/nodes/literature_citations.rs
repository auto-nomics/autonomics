//! `source_literature_citations`: citation-graph retrieval from Semantic
//! Scholar — papers that cite a given paper, or papers it cites — emitted
//! as an `evidence` artifact (the node counterpart of the retired
//! `s2_citations` / `s2_references` tools).

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use bib_types::evidence::{EvidenceRecord, EvidenceSet, FORMAT};
use bib_types::types::Article;
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, DagNode};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::nodes::write_artifact;

/// Which direction of the citation graph to walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CitationDirection {
    /// Papers that cite the given paper (impact / follow-up work).
    #[default]
    Citing,
    /// Papers the given paper cites (its reference list).
    Cited,
}

/// Spec for [`LiteratureCitationsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct LiteratureCitationsSpec {
    /// Paper identifier in any Semantic Scholar form: S2 SHA,
    /// `CorpusId:<id>`, `DOI:<doi>`, `ARXIV:<id>`, `PMID:<id>`,
    /// `PMCID:<id>`, or `URL:<url>`.
    pub paper_id: String,
    /// Graph direction. Default `citing` (papers citing this one).
    #[serde(default)]
    pub direction: CitationDirection,
    /// Maximum records (default 20, hard cap 1000).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Pagination offset (default 0).
    #[serde(default)]
    pub offset: Option<u32>,
    /// Destination for the evidence JSON artifact. `vfs://` URI or absolute
    /// local path; overwritten on re-run.
    pub path: String,
    /// Optional provenance note stamped on every record.
    #[serde(default)]
    pub note: Option<String>,
}

/// Process-wide S2 client: one connection pool for every citations node.
static S2_CLIENT: OnceLock<Arc<semantic_scholar::S2Client>> = OnceLock::new();

pub(crate) fn shared_s2_client() -> Arc<semantic_scholar::S2Client> {
    S2_CLIENT
        .get_or_init(|| Arc::new(semantic_scholar::S2Client::new()))
        .clone()
}

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 1000;

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port_of_type_with_label_and_format(
        None,
        PortType::File,
        "evidence",
        FORMAT,
    )
}

/// Compose the citation-graph metadata (contexts, intents, influence) that
/// has no home in `Article` into a compact `note` prefix, so the evidence
/// record keeps it without widening the payload schema. Shared by the
/// citing and cited directions, which carry identical metadata fields.
pub(crate) fn citation_note(
    contexts: Option<&Vec<String>>,
    intents: Option<&Vec<String>>,
    is_influential: Option<bool>,
    fallback: Option<&str>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if is_influential == Some(true) {
        parts.push("influential citation".into());
    }
    if let Some(intents) = intents.filter(|list| !list.is_empty()) {
        parts.push(format!("intents: [{}]", intents.join(", ")));
    }
    if let Some(contexts) = contexts.filter(|list| !list.is_empty()) {
        // Trim each context snippet to keep the note compact.
        let trimmed: Vec<String> = contexts
            .iter()
            .take(3)
            .map(|context| {
                let snippet: String = context.chars().take(120).collect();
                format!("“{snippet}”")
            })
            .collect();
        parts.push(format!("context: {}", trimmed.join(" ")));
    }
    if let Some(fallback) = fallback {
        parts.push(fallback.to_string());
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

pub struct LiteratureCitationsNode {
    meta: NodePorts,
    spec: LiteratureCitationsSpec,
    /// Test seam: injected client.
    client: Option<Arc<semantic_scholar::S2Client>>,
}

impl LiteratureCitationsNode {
    pub fn new(spec: LiteratureCitationsSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
            client: None,
        }
    }

    /// Build a node bound to a caller-supplied client (tests).
    pub fn with_client(
        spec: LiteratureCitationsSpec,
        client: Arc<semantic_scholar::S2Client>,
    ) -> Self {
        Self {
            meta: port_layout(),
            spec,
            client: Some(client),
        }
    }
}

#[async_trait]
impl DagNode for LiteratureCitationsNode {
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
        "source_literature_citations"
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
        let offset = self.spec.offset.unwrap_or(0);

        let direction_word = match self.spec.direction {
            CitationDirection::Citing => "citing",
            CitationDirection::Cited => "cited",
        };

        let records: Vec<EvidenceRecord> = match self.spec.direction {
            CitationDirection::Citing => {
                let resp = client
                    .get_citations(&self.spec.paper_id, limit, offset, None)
                    .await
                    .map_err(|error| {
                        DagError::Schedule(format!(
                            "citation lookup for `{}` failed: {error}",
                            self.spec.paper_id
                        ))
                    })?;
                resp.data
                    .iter()
                    .map(|entry| EvidenceRecord {
                        citation: semantic_scholar::convert::paper_to_article(&entry.citing_paper),
                        note: citation_note(
                            entry.contexts.as_ref(),
                            entry.intents.as_ref(),
                            entry.is_influential,
                            self.spec.note.as_deref(),
                        ),
                        origin: Some("s2-citations".into()),
                    })
                    .collect()
            }
            CitationDirection::Cited => {
                let resp = client
                    .get_references(&self.spec.paper_id, limit, offset, None)
                    .await
                    .map_err(|error| {
                        DagError::Schedule(format!(
                            "reference lookup for `{}` failed: {error}",
                            self.spec.paper_id
                        ))
                    })?;
                resp.data
                    .iter()
                    .map(|entry| EvidenceRecord {
                        citation: semantic_scholar::convert::paper_to_article(&entry.cited_paper),
                        note: citation_note(
                            entry.contexts.as_ref(),
                            entry.intents.as_ref(),
                            entry.is_influential,
                            self.spec.note.as_deref(),
                        ),
                        origin: Some("s2-references".into()),
                    })
                    .collect()
            }
        };

        if records.is_empty() {
            return Err(DagError::Schedule(format!(
                "no {direction_word} papers returned for `{}` — check the paper id",
                self.spec.paper_id
            )));
        }

        let set = EvidenceSet {
            records,
            ..Default::default()
        };
        let bytes = set.to_bytes().map_err(DagError::Schedule)?;
        let file = write_artifact(node_ctx, &self.spec.path, bytes, FORMAT).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

pub struct LiteratureCitationsNodeFactory {}

#[async_trait]
impl NodeFactory for LiteratureCitationsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_literature_citations"
    }

    fn desc(&self) -> &'static str {
        "Get papers citing a given paper (or its reference list) as an evidence file"
    }

    fn doc(&self) -> &'static str {
        "Walks the Semantic Scholar citation graph from one paper and writes the \
         result as an `evidence` artifact (File, format `evidence`). \
         `direction: citing` (default) returns papers that cite it — impact and \
         follow-up work; `direction: cited` returns its reference list. Each \
         record's note carries the citation-graph metadata: influence flag, \
         intents (methodology / background / result), and up to three context \
         snippets. `paper_id` accepts any S2 form (SHA, `CorpusId:<id>`, \
         `DOI:<doi>`, `ARXIV:<id>`, `PMID:<id>`, `URL:<url>`). Fails when the \
         response is empty — check the id spelling. Chain into evidence_merge \
         with search results, or evidence_export for a bibliography."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LiteratureCitationsSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: LiteratureCitationsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LiteratureCitationsNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(items: &[&str]) -> Option<Vec<String>> {
        Some(items.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn citation_note_composes_metadata_compactly() {
        let contexts = strs(&["We follow the design of X"]);
        let intents = strs(&["methodology"]);
        let note = citation_note(
            contexts.as_ref(),
            intents.as_ref(),
            Some(true),
            Some("user note"),
        )
        .unwrap();
        assert!(
            note.starts_with("influential citation; intents: [methodology]"),
            "{note}"
        );
        assert!(note.contains("context: "), "{note}");
        assert!(note.ends_with("user note"), "{note}");
    }

    #[test]
    fn citation_note_none_when_entry_carries_no_metadata() {
        assert!(citation_note(None, None, None, None).is_none());
        // No metadata but a user note still yields the note.
        assert_eq!(citation_note(None, None, None, Some("n")), Some("n".into()));
    }

    #[test]
    fn article_conversion_preserves_title() {
        let paper = semantic_scholar::types::Paper {
            paper_id: "x".into(),
            title: Some("Follow-up work".into()),
            ..Default::default()
        };
        let article: Article = semantic_scholar::convert::paper_to_article(&paper);
        assert_eq!(article.title, "Follow-up work");
    }
}
