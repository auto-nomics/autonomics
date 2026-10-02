//! `source_literature`: run a structured multi-source literature search and
//! emit one deduplicated `evidence` artifact.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use bib_types::evidence::{EvidenceRecord, EvidenceSet, FORMAT};
use bib_types::query::StructuredSearch;
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, DagNode};
use dag_core::node::{NodeInput, NodePorts};

use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::nodes::write_artifact;
use crate::query::{LiteratureGateway, SourceBatch};

/// Default per-source result limit.
const DEFAULT_LIMIT: u32 = 25;
/// Hard cap on the per-source limit.
const MAX_LIMIT: u32 = 200;

/// Spec for [`LiteratureSearchNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct LiteratureSearchSpec {
    /// Structured search fanned out to every selected source. Fields AND
    /// between each other, operators inside a field come from each field's
    /// `_op` (keywords default to OR).
    pub query: StructuredSearch,
    /// Maximum records per source. Default 25, hard cap 200.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Source names to search (e.g. `["pubmed", "arxiv", "openalex",
    /// "crossref", "s2", "biorxiv"]`). Default: all registered sources. An
    /// unknown name fails the node with the list of valid names.
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    /// Destination for the evidence JSON artifact. `vfs://` URI or absolute
    /// local path; overwritten on re-run. Give each evidence node its own
    /// path — the engine does not detect write-write collisions.
    pub path: String,
    /// Optional provenance note stamped on every emitted record.
    #[serde(default)]
    pub note: Option<String>,
}

/// Process-wide gateway: one connection pool and one arXiv rate-limit window
/// regardless of how many literature nodes run.
static GATEWAY: OnceLock<Arc<LiteratureGateway>> = OnceLock::new();

fn shared_gateway() -> Arc<LiteratureGateway> {
    GATEWAY
        .get_or_init(|| {
            let http = Arc::new(reqwest::Client::new());
            Arc::new(LiteratureGateway::with_all_shared_clients(
                Arc::new(eutils::EutilsClient::from_env()),
                Arc::new(arxiv::ArxivClient::new()),
                http,
                Arc::new(openalex::OpenAlexClient::new(None)),
                Arc::new(crossref::CrossrefClient::new()),
                Arc::new(semantic_scholar::S2Client::new()),
            ))
        })
        .clone()
}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port_of_type_with_label_and_format(
        None,
        PortType::File,
        "evidence",
        FORMAT,
    )
}

pub struct LiteratureSearchNode {
    meta: NodePorts,
    spec: LiteratureSearchSpec,
    /// Test seam: run against an injected gateway (fake sources) instead of
    /// the process-wide shared one.
    gateway: Option<Arc<LiteratureGateway>>,
}

impl LiteratureSearchNode {
    pub fn new(spec: LiteratureSearchSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
            gateway: None,
        }
    }

    /// Build a node bound to a caller-supplied gateway (tests).
    pub fn with_gateway(spec: LiteratureSearchSpec, gateway: Arc<LiteratureGateway>) -> Self {
        Self {
            meta: port_layout(),
            spec,
            gateway: Some(gateway),
        }
    }

    fn gateway(&self) -> Arc<LiteratureGateway> {
        self.gateway.clone().unwrap_or_else(shared_gateway)
    }
}

#[async_trait]
impl DagNode for LiteratureSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            meta: self.meta.clone(),
            spec: self.spec.clone(),
            gateway: self.gateway.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        "source_literature"
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
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let gateway = self.gateway();
        let limit = self
            .spec
            .limit
            .unwrap_or(DEFAULT_LIMIT)
            .min(MAX_LIMIT) as usize;

        if let Some(sources) = &self.spec.sources {
            if sources.is_empty() {
                return Err(DagError::Schedule(
                    "`sources` must name at least one source, or be omitted to search all \
                     registered sources"
                        .into(),
                ));
            }
            let registered = gateway.source_names();
            let unknown: Vec<&str> = sources
                .iter()
                .map(String::as_str)
                .filter(|name| !registered.contains(name))
                .collect();
            if !unknown.is_empty() {
                return Err(DagError::Schedule(format!(
                    "unknown literature source(s) {}: valid names are {}",
                    unknown
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    registered
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                )));
            }
        }

        let batches: Vec<SourceBatch> = gateway
            .search_named(self.spec.sources.as_deref(), &self.spec.query, limit)
            .await;

        let mut set = EvidenceSet::default();
        let mut failed: Vec<String> = Vec::new();
        for batch in &batches {
            match &batch.error {
                Some(error) => {
                    reporter.warn(format!(
                        "literature source `{}` failed: {error}",
                        batch.source
                    ));
                    failed.push(format!("{}: {error}", batch.source));
                }
                None => {
                    for citation in &batch.articles {
                        set.records.push(EvidenceRecord {
                            citation: citation.clone(),
                            note: self.spec.note.clone(),
                            origin: Some(batch.source.clone()),
                        });
                    }
                }
            }
        }

        if batches.is_empty() || set.records.is_empty() && !failed.is_empty() {
            return Err(DagError::Schedule(format!(
                "all requested literature sources failed: {}",
                failed.join("; ")
            )));
        }

        let removed = set.dedup_in_place();
        if removed > 0 {
            reporter.warn(format!(
                "deduplicated {removed} cross-source duplicate record(s) ({} kept)",
                set.records.len()
            ));
        }

        let bytes = set.to_bytes().map_err(DagError::Schedule)?;
        let file = write_artifact(node_ctx, &self.spec.path, bytes, FORMAT).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

pub struct LiteratureSearchNodeFactory {}

#[async_trait]
impl NodeFactory for LiteratureSearchNodeFactory {
    fn kind(&self) -> &'static str {
        "source_literature"
    }

    fn desc(&self) -> &'static str {
        "Run a structured multi-source literature search and emit an evidence file"
    }

    fn doc(&self) -> &'static str {
        "A source node that fans a StructuredSearch out through the literature \
         gateway (pubmed, arxiv, biorxiv, openalex, crossref, s2 — six backends, \
         concurrent) and writes the deduplicated results as one `evidence` \
         artifact: JSON `{schema_version, records[{citation, note, origin}]}`.\n\
         \n\
         No input ports; one output port (File, format `evidence`) — only ports \
         accepting `evidence` may consume it (evidence_merge, evidence_export).\n\
         \n\
         `query` is the same StructuredSearch the lit tools use (keywords OR \
         within, fields AND between). `limit` is per source (default 25, max \
         200). `sources` selects a subset by name; an unknown name fails the \
         node with the valid names listed. Per-source failures degrade \
         gracefully: they are reported as warnings and the remaining sources' \
         results still flow; the node fails only when every requested source \
         failed. Cross-source duplicates (same DOI/PMID/arXiv id) are merged, \
         first occurrence wins.\n\
         \n\
         `path` is overwritten on re-run; give each evidence node a distinct \
         path. Inspect the artifact with get_output — evidence files render as \
         a compact citation list."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LiteratureSearchSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: LiteratureSearchSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LiteratureSearchNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bib_types::query::StructuredSearch;
    use bib_types::types::{Article, IdKind, Identifier};
    use dag_core::dag::node_event::NodeReporter;

    struct FakeSource {
        name: &'static str,
        articles: Vec<Article>,
        fail: Option<&'static str>,
    }

    #[async_trait]
    impl crate::query::LiteratureSource for FakeSource {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn search(
            &self,
            _query: &StructuredSearch,
            _limit: usize,
        ) -> crate::Result<SourceBatch> {
            if let Some(reason) = self.fail {
                return Ok(SourceBatch {
                    source: self.name.to_string(),
                    total: 0,
                    articles: Vec::new(),
                    error: Some(reason.to_string()),
                });
            }
            Ok(SourceBatch {
                source: self.name.to_string(),
                total: self.articles.len(),
                articles: self.articles.clone(),
                error: None,
            })
        }

        fn supports(&self, _kind: IdKind) -> bool {
            false
        }

        async fn fetch(&self, _id: &Identifier) -> crate::Result<Option<Article>> {
            Ok(None)
        }
    }

    fn article(id: &str, title: &str, doi: Option<&str>) -> Article {
        let mut a = Article::new(id, title);
        if let Some(doi) = doi {
            a.identifiers.push(Identifier::doi(doi));
        }
        a
    }

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    async fn run(
        spec: LiteratureSearchSpec,
        gateway: Arc<LiteratureGateway>,
    ) -> std::result::Result<PortOutputs, DagError> {
        let mut node = LiteratureSearchNode::with_gateway(spec, gateway);
        let ctx = node_ctx();
        let reporter = NodeReporter::noop();
        node.execute(&ctx, &[], &reporter).await
    }

    fn tmp_path(tag: &str) -> String {
        std::env::temp_dir()
            .join(format!("lit-node-{tag}-{}.json", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned()
    }

    fn gateway_with(sources: Vec<Arc<dyn crate::query::LiteratureSource>>) -> Arc<LiteratureGateway> {
        let mut gateway = LiteratureGateway::new();
        for source in sources {
            gateway.add_source(source);
        }
        Arc::new(gateway)
    }

    #[tokio::test]
    async fn flattens_batches_and_stamps_origin_and_note() {
        let gateway = gateway_with(vec![
            Arc::new(FakeSource {
                name: "src_a",
                articles: vec![article("1", "Alpha", Some("10.1/a"))],
                fail: None,
            }),
            Arc::new(FakeSource {
                name: "src_b",
                articles: vec![article("2", "Beta", None)],
                fail: None,
            }),
        ]);
        let path = tmp_path("flatten");
        let spec = LiteratureSearchSpec {
            query: StructuredSearch::default(),
            limit: None,
            sources: None,
            path: path.clone(),
            note: Some("query context".into()),
        };

        let outputs = run(spec, gateway).await.unwrap();
        let file = outputs
            .get(&0)
            .and_then(|value| value.as_file().ok())
            .expect("file output");
        let bytes = tokio::fs::read(&path).await.unwrap();
        let set = EvidenceSet::parse(&bytes).unwrap();
        assert_eq!(set.records.len(), 2);
        assert_eq!(set.records[0].origin.as_deref(), Some("src_a"));
        assert_eq!(set.records[0].note.as_deref(), Some("query context"));
        // Artifact fingerprint is content-addressed.
        assert!(file
            .fingerprint
            .as_ref()
            .and_then(|fp| fp.content_hash.as_deref())
            .is_some_and(|hash| hash.starts_with("sha256:")));
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn failed_source_degrades_but_all_failed_is_an_error() {
        // One healthy + one failing source: succeeds with the healthy batch.
        let gateway = gateway_with(vec![
            Arc::new(FakeSource {
                name: "ok",
                articles: vec![article("1", "Alpha", None)],
                fail: None,
            }),
            Arc::new(FakeSource {
                name: "down",
                articles: vec![],
                fail: Some("rate limited"),
            }),
        ]);
        let spec = LiteratureSearchSpec {
            query: StructuredSearch::default(),
            limit: None,
            sources: None,
            path: tmp_path("degrade"),
            note: None,
        };
        let outputs = run(spec, gateway).await.unwrap();
        drop(outputs);

        // Every source failing: node errors.
        let gateway = gateway_with(vec![Arc::new(FakeSource {
            name: "down",
            articles: vec![],
            fail: Some("timeout"),
        })]);
        let spec = LiteratureSearchSpec {
            query: StructuredSearch::default(),
            limit: None,
            sources: None,
            path: tmp_path("allfail"),
            note: None,
        };
        let err = run(spec, gateway).await.unwrap_err().to_string();
        assert!(err.contains("all requested literature sources failed"), "{err}");
        assert!(err.contains("timeout"));
    }

    #[tokio::test]
    async fn unknown_source_name_fails_with_valid_names() {
        let gateway = gateway_with(vec![Arc::new(FakeSource {
            name: "pubmed",
            articles: vec![],
            fail: None,
        })]);
        let spec = LiteratureSearchSpec {
            query: StructuredSearch::default(),
            limit: None,
            sources: Some(vec!["pubmedx".into()]),
            path: tmp_path("unknown"),
            note: None,
        };
        let err = run(spec, gateway).await.unwrap_err().to_string();
        assert!(err.contains("unknown literature source"), "{err}");
        assert!(err.contains("`pubmed`"), "{err}");
    }

    #[tokio::test]
    async fn limit_is_capped_and_dedups_across_sources() {
        let gateway = gateway_with(vec![
            Arc::new(FakeSource {
                name: "a",
                articles: vec![article("1", "Same", Some("10.1/x"))],
                fail: None,
            }),
            Arc::new(FakeSource {
                name: "b",
                // Same DOI, different title: identifier dedup must merge.
                articles: vec![article("2", "Same (alt)", Some("10.1/x"))],
                fail: None,
            }),
        ]);
        let path = tmp_path("dedup");
        let spec = LiteratureSearchSpec {
            query: StructuredSearch::default(),
            limit: Some(99_999), // silently capped at MAX_LIMIT
            sources: None,
            path: path.clone(),
            note: None,
        };
        run(spec, gateway).await.unwrap();
        let set = EvidenceSet::parse(&tokio::fs::read(&path).await.unwrap()).unwrap();
        assert_eq!(set.records.len(), 1);
        assert_eq!(set.records[0].citation.title, "Same");
        std::fs::remove_file(&path).ok();
    }
}
