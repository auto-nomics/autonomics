//! `source_literature_fetch`: retrieve one article by typed identifier and
//! emit it as a single-record `evidence` artifact (the node counterpart of
//! the retired `lit_fetch` tool).

use std::sync::Arc;

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use bib_types::evidence::{EvidenceRecord, EvidenceSet, FORMAT};
use bib_types::types::{Article, IdKind, Identifier};
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, DagNode};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::nodes::literature_search::shared_gateway;
use crate::nodes::write_artifact;
use crate::query::LiteratureGateway;

/// Spec for [`LiteratureFetchNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct LiteratureFetchSpec {
    /// Identifier kind: `doi`, `pmid`, `pmc`, `embase`, `arxiv`, `biorxiv`,
    /// `s2`, `openalex`, or `other`.
    pub id_kind: String,
    /// The identifier value (DOIs are normalized automatically).
    pub id_value: String,
    /// Sources to try, in order (e.g. `["pubmed", "crossref"]`). Default:
    /// every source that supports the identifier kind, in registration
    /// order, first hit wins. An unknown name fails the node.
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    /// Destination for the evidence JSON artifact. `vfs://` URI or absolute
    /// local path; overwritten on re-run.
    pub path: String,
    /// Optional provenance note stamped on the record.
    #[serde(default)]
    pub note: Option<String>,
}

fn parse_id_kind(raw: &str) -> Result<IdKind, DagError> {
    let normalized = raw.trim().to_lowercase();
    let kind = match normalized.as_str() {
        "doi" => IdKind::Doi,
        "pmid" => IdKind::Pmid,
        "pmc" => IdKind::Pmc,
        "embase" => IdKind::Embase,
        "arxiv" => IdKind::Arxiv,
        "biorxiv" => IdKind::Biorxiv,
        "s2" => IdKind::S2,
        "openalex" => IdKind::OpenAlex,
        "other" => IdKind::Other,
        other => {
            return Err(DagError::Schedule(format!(
                "unknown id_kind `{other}`; valid kinds: doi, pmid, pmc, embase, arxiv, \
                 biorxiv, s2, openalex, other"
            )))
        }
    };
    Ok(kind)
}

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port_of_type_with_label_and_format(
        None,
        PortType::File,
        "evidence",
        FORMAT,
    )
}

pub struct LiteratureFetchNode {
    meta: NodePorts,
    spec: LiteratureFetchSpec,
    /// Test seam: run against an injected gateway instead of the shared one.
    gateway: Option<Arc<LiteratureGateway>>,
}

impl LiteratureFetchNode {
    pub fn new(spec: LiteratureFetchSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
            gateway: None,
        }
    }

    /// Build a node bound to a caller-supplied gateway (tests).
    pub fn with_gateway(spec: LiteratureFetchSpec, gateway: Arc<LiteratureGateway>) -> Self {
        Self {
            meta: port_layout(),
            spec,
            gateway: Some(gateway),
        }
    }
}

#[async_trait]
impl DagNode for LiteratureFetchNode {
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
        "source_literature_fetch"
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
        let gateway = self
            .gateway
            .clone()
            .unwrap_or_else(shared_gateway);
        let kind = parse_id_kind(&self.spec.id_kind)?;
        let value = if kind == IdKind::Doi {
            bib_types::convert::normalize_doi(&self.spec.id_value)
        } else {
            self.spec.id_value.trim().to_string()
        };
        let identifier = Identifier::new(kind, value.clone());

        let fetched: Option<(String, Article)> = match &self.spec.sources {
            None => gateway.fetch(&identifier).await,
            Some(sources) => {
                if sources.is_empty() {
                    return Err(DagError::Schedule(
                        "`sources` must name at least one source, or be omitted".into(),
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
                        "unknown literature source(s) {}; valid names: {}",
                        unknown.join(", "),
                        registered.join(", ")
                    )));
                }
                let mut hit = None;
                for name in sources {
                    match gateway.fetch_from(name, &identifier).await {
                        Ok(Some(article)) => {
                            hit = Some((name.clone(), article));
                            break;
                        }
                        // A source that errors on this identifier is skipped,
                        // matching the try-in-order semantics of `fetch`.
                        Ok(None) | Err(_) => continue,
                    }
                }
                hit
            }
        };

        let Some((source_name, citation)) = fetched else {
            return Err(DagError::Schedule(format!(
                "no literature source returned an article for {} `{value}`",
                kind.as_str()
            )));
        };

        let set = EvidenceSet {
            records: vec![EvidenceRecord {
                citation,
                note: self.spec.note.clone(),
                origin: Some(source_name),
            }],
            ..Default::default()
        };
        let bytes = set.to_bytes().map_err(DagError::Schedule)?;
        let file = write_artifact(node_ctx, &self.spec.path, bytes, FORMAT).await?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }
}

pub struct LiteratureFetchNodeFactory {}

#[async_trait]
impl NodeFactory for LiteratureFetchNodeFactory {
    fn kind(&self) -> &'static str {
        "source_literature_fetch"
    }

    fn desc(&self) -> &'static str {
        "Retrieve one article by DOI / PMID / arXiv / S2 / OpenAlex id as an evidence file"
    }

    fn doc(&self) -> &'static str {
        "Fetches a single bibliographic record by typed identifier through the \
         literature gateway and writes it as a one-record `evidence` artifact \
         (File, format `evidence`). Sources that support the identifier kind \
         are tried in registration order, first hit wins; pass `sources` to \
         pin an order. Fails when no source knows the identifier. Chain into \
         evidence_merge to combine with search results, or evidence_export \
         for a bibliography. DOI values are normalized (URL prefixes \
         stripped) automatically."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LiteratureFetchSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: LiteratureFetchSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LiteratureFetchNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{LiteratureGateway, LiteratureSource, SourceBatch};
    use bib_types::query::StructuredSearch;
    use dag_core::dag::node_event::NodeReporter;

    struct FetchableSource {
        article: Option<Article>,
    }

    #[async_trait]
    impl LiteratureSource for FetchableSource {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn search(
            &self,
            _query: &StructuredSearch,
            _limit: usize,
        ) -> crate::Result<SourceBatch> {
            Ok(SourceBatch {
                source: "fake".into(),
                total: 0,
                articles: Vec::new(),
                error: None,
            })
        }

        async fn fetch(&self, id: &Identifier) -> crate::Result<Option<Article>> {
            // DOIs are case-insensitive at real APIs; compare lowercased.
            Ok(match id.kind {
                IdKind::Doi if id.value.to_lowercase() == "10.1/known" => self.article.clone(),
                _ => None,
            })
        }
    }

    fn spec(id_kind: &str, id_value: &str, path: &str) -> LiteratureFetchSpec {
        LiteratureFetchSpec {
            id_kind: id_kind.into(),
            id_value: id_value.into(),
            sources: None,
            path: path.into(),
            note: Some("ctx".into()),
        }
    }

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(datafusion::prelude::SessionContext::new().runtime_env(), None)
    }

    fn tmp_path(tag: &str) -> String {
        std::env::temp_dir()
            .join(format!("fetch-node-{tag}-{}.json", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned()
    }

    #[tokio::test]
    async fn fetches_known_doi_into_single_record_evidence() {
        let mut article = Article::new("1", "Known paper");
        article.identifiers.push(Identifier::doi("10.1/known"));
        let mut gateway = LiteratureGateway::new();
        gateway.add_source(Arc::new(FetchableSource {
            article: Some(article),
        }));
        let gateway = Arc::new(gateway);
        let path = tmp_path("known");
        let mut node = LiteratureFetchNode::with_gateway(
            spec("DOI", "https://doi.org/10.1/KNOWN", &path),
            gateway,
        );
        let outputs = node
            .execute(&node_ctx(), &[], &NodeReporter::noop())
            .await
            .unwrap();
        assert!(outputs
            .get(&0)
            .and_then(|value| value.as_file().ok())
            .is_some_and(|file| file.format.as_deref() == Some("evidence")));

        let set = EvidenceSet::parse(&tokio::fs::read(&path).await.unwrap()).unwrap();
        assert_eq!(set.records.len(), 1);
        assert_eq!(set.records[0].citation.title, "Known paper");
        assert_eq!(set.records[0].origin.as_deref(), Some("fake"));
        assert_eq!(set.records[0].note.as_deref(), Some("ctx"));
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn unknown_identifier_fails_closed() {
        let mut gateway = LiteratureGateway::new();
        gateway.add_source(Arc::new(FetchableSource { article: None }));
        let gateway = Arc::new(gateway);
        let mut node = LiteratureFetchNode::with_gateway(
            spec("doi", "10.1/missing", &tmp_path("missing")),
            gateway,
        );
        let err = node
            .execute(&node_ctx(), &[], &NodeReporter::noop())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no literature source returned"), "{err}");
    }

    #[tokio::test]
    async fn unknown_id_kind_lists_valid_kinds() {
        let mut node = LiteratureFetchNode::new(spec("isbn", "x", &tmp_path("kind")));
        let err = node
            .execute(&node_ctx(), &[], &NodeReporter::noop())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown id_kind `isbn`"), "{err}");
        assert!(err.contains("openalex"), "{err}");
    }
}
