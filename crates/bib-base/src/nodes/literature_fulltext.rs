//! `literature_fulltext`: resolve the full-text files behind evidence
//! records into a `FileSet` of real VFS `FileRef`s.
//!
//! The bibliography library keeps uploaded full texts as content-addressed
//! VFS objects (`vfs:///literature/{article_id}/{sha256}-{filename}`) with
//! `fulltexts` DB rows as pointers. This node turns evidence records into
//! those pointers' [`FileRef`]s — identity comes straight from the DB row
//! (`file_hash` + `file_size`), so no file I/O is needed for fingerprinting.
//!
//! One repair on the way through: full texts fetched from Europe PMC are
//! stored as synthetic pointers (`europepmc:PMC…`) with the text inline and
//! no VFS object behind them. The node materializes those into real
//! content-addressed objects and rewrites the row, so every resolved output
//! is an actual file — get_output shows a FileRef, not a URI scheme.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use bib_types::evidence::{EvidenceSet, FORMAT};
use bib_types::types::{FileFormat, FullText, IdKind};
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, DagNode};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::{FileFingerprint, FileRef, PortType};

use crate::nodes::{read_file_bytes, shared_bib};
use crate::oa_fetch::fetch_fulltext_stored;
use crate::shared::BibShared;
use crate::stored_files::{stored_fulltext, vfs_virtual_path};

/// Spec for [`LiteratureFulltextNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct LiteratureFulltextSpec {
    /// When `true` (default), records whose article is missing from the
    /// library — or present without a full text — are saved and fetched
    /// from Europe PMC open access, mirroring `bib_save`. When `false` the
    /// node is a pure resolver over the existing library.
    #[serde(default)]
    pub fetch_missing: Option<bool>,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label_and_format(None, PortType::File, "evidence", FORMAT)
        .add_output_port_of_type(None, PortType::FileSet)
}

/// Build the output [`FileRef`] from what the DB row already records:
/// content-addressed VFS objects are immutable, so the row's hash and size
/// are a permanent fingerprint with no stat or read required.
fn file_ref(fulltext: &FullText) -> FileRef {
    FileRef {
        path: fulltext.file_path.clone(),
        format: Some(fulltext.file_format.as_str().to_string()),
        fingerprint: Some(FileFingerprint {
            size: fulltext.file_size.unwrap_or_default() as u64,
            mtime_ns: 0,
            content_hash: fulltext
                .file_hash
                .as_ref()
                .map(|hash| format!("sha256:{hash}")),
            immutable_remote: true,
        }),
    }
}

pub struct LiteratureFulltextNode {
    meta: NodePorts,
    spec: LiteratureFulltextSpec,
    /// Test seam: injected shared bibliography.
    shared: Option<Arc<BibShared>>,
}

impl LiteratureFulltextNode {
    pub fn new(spec: LiteratureFulltextSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
            shared: None,
        }
    }

    /// Build a node bound to a caller-supplied bibliography (tests).
    pub fn with_shared(spec: LiteratureFulltextSpec, shared: Arc<BibShared>) -> Self {
        Self {
            meta: port_layout(),
            spec,
            shared: Some(shared),
        }
    }
}

/// Identifier kinds tried, in order, when matching an evidence record
/// against the library — same priority the dedup layer uses.
const ID_KINDS: [IdKind; 4] = [IdKind::Doi, IdKind::Pmid, IdKind::Pmc, IdKind::Arxiv];

impl LiteratureFulltextNode {
    fn shared(&self) -> Result<Arc<BibShared>, DagError> {
        self.shared.clone().map(Ok).unwrap_or_else(shared_bib)
    }

    /// Resolve one evidence record to a stored [`FullText`], optionally
    /// saving + OA-fetching when missing. `None` with a reason on miss.
    async fn resolve_record(
        &self,
        bib: &Arc<crate::bib_base::BibBase>,
        shared: &Arc<BibShared>,
        record: &bib_types::evidence::EvidenceRecord,
        fetch_missing: bool,
    ) -> Result<Option<FullText>, String> {
        let citation = &record.citation;
        let describe = || {
            citation
                .doi()
                .map(|doi| format!("doi:{doi}"))
                .or_else(|| citation.pmid().map(|pmid| format!("pmid:{pmid}")))
                .unwrap_or_else(|| citation.title.clone())
        };

        // 1. Match the record against the library by any shared identifier.
        // The `identifiers` table compares exactly, so DOIs (case-insensitive
        // in the wild) are tried in the source casing and lowercased.
        let mut article_id = None;
        'outer: for kind in ID_KINDS {
            let Some(value) = citation.identifier(kind) else {
                continue;
            };
            let normalized = if kind == IdKind::Doi {
                bib_types::convert::normalize_doi(value)
            } else {
                value.trim().to_string()
            };
            let mut candidates = vec![normalized.clone()];
            if kind == IdKind::Doi {
                let lower = normalized.to_lowercase();
                if lower != normalized {
                    candidates.push(lower);
                }
            }
            for candidate in &candidates {
                if let Ok(Some(article)) = bib.find_by_identifier(kind, candidate).await {
                    article_id = Some(article.id);
                    break 'outer;
                }
            }
        }

        // 2. Missing article: save the evidence citation itself, then fetch.
        if article_id.is_none() && fetch_missing {
            bib.upsert_article(citation)
                .await
                .map_err(|error| format!("saving `{}`: {error}", describe()))?;
            article_id = Some(citation.id.clone());
        }
        let Some(article_id) = article_id else {
            return Ok(None);
        };

        // 3. Existing full text.
        if let Ok(Some(fulltext)) = bib.get_fulltext(&article_id).await {
            return Ok(Some(fulltext));
        }
        if !fetch_missing {
            return Ok(None);
        }

        // 4. OA fetch — the canonical file-writing path (same chain as
        // bib_save): text lands in the VFS, the row points at the file.
        let Some(article) = bib
            .get_article(&article_id)
            .await
            .map_err(|error| format!("reloading `{}`: {error}", describe()))?
        else {
            return Ok(None);
        };
        let fetched = match shared.file_storage.as_ref() {
            Some(storage) => {
                fetch_fulltext_stored(&shared.europe_pmc, storage, &article).await
            }
            // No storage handle: fetching would produce a synthetic
            // pointer, which this node exists to eliminate.
            None => return Ok(None),
        };
        match fetched {
            Some(mut fulltext) => {
                fulltext.article_id = article_id;
                bib.upsert_fulltext(&fulltext)
                    .await
                    .map_err(|error| format!("storing full text for `{}`: {error}", describe()))?;
                Ok(Some(fulltext))
            }
            None => Ok(None),
        }
    }

    /// Synthetic `europepmc:` pointers carry their text inline with no VFS
    /// object. Materialize the text into a content-addressed object,
    /// rewrite the row, and return the real path.
    async fn materialize(
        &self,
        bib: &Arc<crate::bib_base::BibBase>,
        storage: &Arc<vfs::OpendalFileStorage>,
        mut fulltext: FullText,
    ) -> Result<FullText, DagError> {
        let text = fulltext.text_content.clone().unwrap_or_default();
        if text.is_empty() {
            return Err(DagError::Schedule(format!(
                "full text for `{}` is a synthetic pointer with no inline text",
                fulltext.article_id
            )));
        }
        let stored = stored_fulltext(
            &fulltext.article_id,
            &format!("oa-{}.txt", fulltext.article_id),
            text.as_bytes(),
        );
        let virtual_path =
            vfs_virtual_path(&stored.path).ok_or_else(|| {
                DagError::Schedule(format!("invalid stored path `{}`", stored.path))
            })?;
        let byte_len = text.len();
        storage
            .write_bytes(&virtual_path, text.into_bytes())
            .await
            .map_err(|error| {
                DagError::Schedule(format!(
                    "cannot materialize full text for `{}`: {error}",
                    fulltext.article_id
                ))
            })?;
        fulltext.file_path = stored.path;
        fulltext.file_format = FileFormat::Txt;
        fulltext.file_hash = Some(stored.file_hash.clone());
        fulltext.file_size = Some(byte_len as i64);
        bib.upsert_fulltext(&fulltext).await.map_err(|error| {
            DagError::Schedule(format!(
                "cannot record materialized full text for `{}`: {error}",
                fulltext.article_id
            ))
        })?;
        Ok(fulltext)
    }
}

#[async_trait]
impl DagNode for LiteratureFulltextNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            meta: self.meta.clone(),
            spec: self.spec.clone(),
            shared: self.shared.clone(),
        })
    }

    fn kind(&self) -> &'static str {
        "literature_fulltext"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| {
            DagError::Schedule("literature_fulltext requires exactly one evidence input".into())
        })?;
        let file = input.file_value()?;
        if let Some(format) = file.format.as_deref()
            && format != FORMAT
        {
            return Err(DagError::Schedule(format!(
                "literature_fulltext input carries format `{format}`, expected `{FORMAT}`"
            )));
        }

        let shared = self.shared()?;
        let bib = shared.bib.clone();
        let bytes = read_file_bytes(node_ctx, &file.path).await?;
        let set = EvidenceSet::parse(&bytes).map_err(|error| {
            DagError::Schedule(format!("literature_fulltext input `{}`: {error}", file.path))
        })?;
        let fetch_missing = self.spec.fetch_missing.unwrap_or(true);

        let mut files: Vec<FileRef> = Vec::with_capacity(set.records.len());
        let mut misses: Vec<String> = Vec::new();
        for record in &set.records {
            match self.resolve_record(&bib, &shared, record, fetch_missing).await {
                Ok(Some(fulltext)) => {
                    let fulltext = if fulltext.file_path.starts_with("vfs://") {
                        fulltext
                    } else if let Some(storage) = shared.file_storage.as_ref() {
                        // Synthetic pointer: materialize once, then it is a
                        // real object for every future run too.
                        self.materialize(&bib, storage, fulltext).await?
                    } else {
                        reporter.warn(format!(
                            "full text for article `{}` is a synthetic pointer and no VFS \
                             storage is attached; skipping",
                            fulltext.article_id
                        ));
                        continue;
                    };
                    files.push(file_ref(&fulltext));
                }
                Ok(None) => misses.push(miss_label(record)),
                Err(reason) => {
                    return Err(DagError::Schedule(format!(
                        "literature_fulltext failed on {reason}"
                    )))
                }
            }
        }

        if files.is_empty() {
            return Err(DagError::Schedule(format!(
                "no full text resolved for any of the {} evidence record(s): {}",
                set.records.len(),
                misses.join(", ")
            )));
        }
        if !misses.is_empty() {
            reporter.warn(format!(
                "{} of {} evidence record(s) have no full text available: {}",
                misses.len(),
                set.records.len(),
                misses.join(", ")
            ));
        }

        let mut outputs = PortOutputs::new();
        outputs.insert(0, files);
        Ok(outputs)
    }
}

fn miss_label(record: &bib_types::evidence::EvidenceRecord) -> String {
    let citation = &record.citation;
    citation
        .doi()
        .map(|doi| format!("doi:{}", bib_types::convert::normalize_doi(doi)))
        .or_else(|| citation.pmid().map(|pmid| format!("pmid:{pmid}")))
        .unwrap_or_else(|| citation.short_cite())
}

pub struct LiteratureFulltextNodeFactory {}

#[async_trait]
impl NodeFactory for LiteratureFulltextNodeFactory {
    fn kind(&self) -> &'static str {
        "literature_fulltext"
    }

    fn desc(&self) -> &'static str {
        "Resolve evidence records to their full-text files as a FileSet of FileRefs"
    }

    fn doc(&self) -> &'static str {
        "Takes one `evidence` artifact and resolves every record to the full-text \
         file stored in the bibliography library, emitting a FileSet of real \
         VFS FileRefs — get_output on this node lists the actual files \
         (path, format pdf/txt/html, sha256 fingerprint). Records are matched \
         to the library by DOI, then PMID, PMCID, arXiv id. With \
         `fetch_missing: true` (default), unmatched records are saved from \
         the evidence citation and their full text fetched from Europe PMC \
         open access **as a stored VFS file** (the same file-writing chain \
         as bib_save); `false` makes the node a pure resolver over the \
         existing library.\n\
         \n\
         Open-access texts that were stored as synthetic pointers without a \
         backing file are materialized into content-addressed VFS objects on \
         first resolution. Records with no full text available are skipped \
         with a warning naming the identifier; the node fails only when \
         nothing resolves at all.\n\
         \n\
         Requires the runtime host's shared bibliography (the same storage \
         the engine and the library mount)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LiteratureFulltextSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: LiteratureFulltextSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LiteratureFulltextNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bib_types::evidence::EvidenceRecord;
    use bib_types::types::{Article, Identifier};
    use dag_core::dag::node_event::NodeReporter;
    use dag_core::node::NodeInput;

    async fn shared_with_storage() -> Arc<BibShared> {
        let shared = BibShared::open_in_memory()
            .await
            .unwrap()
            .with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp()));
        Arc::new(shared)
    }

    fn node_ctx() -> NodeCtx {
        NodeCtx::new(datafusion::prelude::SessionContext::new().runtime_env(), None)
    }

    async fn write_evidence(tag: &str, records: Vec<EvidenceRecord>) -> String {
        let path = std::env::temp_dir()
            .join(format!("fulltext-node-{tag}-{}.json", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        let set = EvidenceSet {
            records,
            ..Default::default()
        };
        tokio::fs::write(&path, set.to_bytes().unwrap()).await.unwrap();
        path
    }

    async fn upload_fulltext(shared: &BibShared, article_id: &str, content: &[u8]) -> FullText {
        let stored = stored_fulltext(article_id, "paper.pdf", content);
        let virtual_path = vfs_virtual_path(&stored.path).unwrap();
        shared
            .file_storage
            .as_ref()
            .unwrap()
            .write_bytes(&virtual_path, content.to_vec())
            .await
            .unwrap();
        let fulltext = FullText {
            article_id: article_id.to_owned(),
            file_path: stored.path,
            file_format: FileFormat::Pdf,
            text_content: None,
            source: bib_types::types::FullTextSource::UserUpload,
            file_hash: Some(stored.file_hash.clone()),
            file_size: Some(content.len() as i64),
            uploaded_at: None,
            extract_status: None,
            text_format: None,
            extracted_by: None,
            extract_error: None,
        };
        shared.bib.upsert_fulltext(&fulltext).await.unwrap();
        fulltext
    }

    #[tokio::test]
    async fn resolves_uploaded_fulltext_to_fileset() {
        let shared = shared_with_storage().await;
        let mut article = Article::new("art-1", "Uploaded study");
        article.identifiers.push(Identifier::doi("10.1/up"));
        shared.bib.upsert_article(&article).await.unwrap();
        let stored = upload_fulltext(&shared, "art-1", b"%PDF-1.4 fake").await;

        let mut record_citation = Article::new("ev-1", "Uploaded study");
        record_citation.identifiers.push(Identifier::doi("https://doi.org/10.1/UP"));
        let path = write_evidence(
            "uploaded",
            vec![EvidenceRecord {
                citation: record_citation,
                note: None,
                origin: None,
            }],
        )
        .await;

        let mut node = LiteratureFulltextNode::with_shared(
            LiteratureFulltextSpec { fetch_missing: Some(false) },
            shared.clone(),
        );
        let inputs = vec![NodeInput::file(0, FileRef::local(&path, Some(FORMAT.into())).unwrap())];
        let outputs = node
            .execute(&node_ctx(), &inputs, &NodeReporter::noop())
            .await
            .unwrap();

        let files = outputs
            .get(&0)
            .and_then(|value| value.as_file_set().ok())
            .expect("FileSet output");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, stored.file_path);
        assert_eq!(files[0].format.as_deref(), Some("pdf"));
        let fingerprint = files[0].fingerprint.as_ref().unwrap();
        assert_eq!(fingerprint.size, b"%PDF-1.4 fake".len() as u64);
        assert_eq!(
            fingerprint.content_hash.as_deref(),
            stored.file_hash.map(|hash| format!("sha256:{hash}")).as_deref()
        );
        assert!(fingerprint.immutable_remote);
        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn materializes_synthetic_pointer_into_vfs_object() {
        let shared = shared_with_storage().await;
        let mut article = Article::new("art-2", "OA study");
        article.identifiers.push(Identifier::pmid("42"));
        shared.bib.upsert_article(&article).await.unwrap();
        // Synthetic Europe PMC pointer: inline text, no VFS object.
        shared
            .bib
            .upsert_fulltext(&FullText {
                article_id: "art-2".into(),
                file_path: "europepmc:PMC42".into(),
                file_format: FileFormat::Txt,
                text_content: Some("full text words".into()),
                source: bib_types::types::FullTextSource::OpenAccess,
                file_hash: None,
                file_size: Some(15),
                uploaded_at: None,
                extract_status: None,
                text_format: None,
                extracted_by: None,
                extract_error: None,
            })
            .await
            .unwrap();

        let path = write_evidence(
            "synthetic",
            vec![EvidenceRecord {
                citation: article.clone(),
                note: None,
                origin: None,
            }],
        )
        .await;

        let mut node = LiteratureFulltextNode::with_shared(
            LiteratureFulltextSpec { fetch_missing: Some(false) },
            shared.clone(),
        );
        let inputs = vec![NodeInput::file(0, FileRef::local(&path, Some(FORMAT.into())).unwrap())];
        let outputs = node
            .execute(&node_ctx(), &inputs, &NodeReporter::noop())
            .await
            .unwrap();

        let files = outputs
            .get(&0)
            .and_then(|value| value.as_file_set().ok())
            .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].path.starts_with("vfs:///literature/art-2/"), "{}", files[0].path);
        assert!(files[0]
            .fingerprint
            .as_ref()
            .and_then(|fp| fp.content_hash.as_deref())
            .is_some_and(|hash| hash.starts_with("sha256:")));

        // The DB row now points at the materialized object.
        let row = shared.bib.get_fulltext("art-2").await.unwrap().unwrap();
        assert_eq!(row.file_path, files[0].path);
        // And the object is readable through the library storage.
        let virtual_path = vfs_virtual_path(&row.file_path).unwrap();
        let bytes = shared
            .file_storage
            .as_ref()
            .unwrap()
            .op
            .read(&virtual_path)
            .await
            .unwrap();
        assert_eq!(bytes.to_vec(), b"full text words");
        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn nothing_resolved_fails_with_identifier_list() {
        let shared = shared_with_storage().await;
        let mut citation = Article::new("ev-x", "Unknown study");
        citation.identifiers.push(Identifier::doi("10.1/nowhere"));
        let path = write_evidence(
            "miss",
            vec![EvidenceRecord {
                citation,
                note: None,
                origin: None,
            }],
        )
        .await;
        let mut node = LiteratureFulltextNode::with_shared(
            LiteratureFulltextSpec { fetch_missing: Some(false) },
            shared,
        );
        let inputs = vec![NodeInput::file(0, FileRef::local(&path, Some(FORMAT.into())).unwrap())];
        let err = node
            .execute(&node_ctx(), &inputs, &NodeReporter::noop())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no full text resolved"), "{err}");
        assert!(err.contains("doi:10.1/nowhere"), "{err}");
        std::fs::remove_file(path).ok();
    }

    #[tokio::test]
    async fn without_shared_bibliography_fails_closed() {
        let mut node = LiteratureFulltextNode::new(LiteratureFulltextSpec { fetch_missing: None });
        let err = node
            .execute(&node_ctx(), &[], &NodeReporter::noop())
            .await
            .unwrap_err()
            .to_string();
        // Empty input trips first; call the accessor directly for the
        // no-shared case.
        drop(err);
        let err = shared_bib().unwrap_err().to_string();
        assert!(err.contains("shared bibliography"), "{err}");
    }
}
