//! `bib_save`: import evidence citations into the bibliography library.

use std::sync::Arc;

use arrow_array::{BooleanArray, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use bib_types::evidence::{EvidenceSet, FORMAT, cite_key};
use bib_types::types::{ArticleRole, IdKind};
use bib_types::{AddedBy, Article};
use dag_core::dag::graph::PortOutputs;
use dag_core::dag::{DagError, DagNode};
use dag_core::node::{NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

use crate::collections::{CollectionAddOutcome, CollectionAssignment};
use crate::nodes::read_file_bytes;
use crate::nodes::shared_bib;
use crate::oa_fetch::fetch_fulltext_stored;
use crate::shared::BibShared;

/// Existing-library handling for one evidence record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExistingPolicy {
    /// Do not modify an already-saved article.
    #[default]
    Skip,
    /// Refresh metadata while preserving annotations, full texts, and
    /// collection memberships.
    Refresh,
    /// Refuse the whole run when an input record is already saved.
    Fail,
}

/// Validation/write granularity for the evidence batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    /// Validate every accepted record, then commit metadata and collection
    /// changes in one database transaction.
    #[default]
    Atomic,
    /// Skip invalid records and commit the valid remainder.
    BestEffort,
}

/// How existing collection notes are handled when re-linking an article.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionNotePolicy {
    /// Preserve the existing note unless an explicit note is supplied.
    #[default]
    Preserve,
    /// Overwrite the existing note with `note` when supplied.
    Overwrite,
    /// Use the evidence record's note as the collection note when present.
    FromEvidence,
}

/// Which successfully resolved records join the optional collection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionScope {
    /// New and already-known articles both join the collection.
    #[default]
    AllMatched,
    /// Only articles inserted by this run join the collection.
    NewOnly,
}

/// Optional collection association. The collection must already exist.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct BibSaveCollectionSpec {
    /// Existing collection ID. Names are deliberately not accepted.
    pub id: String,
    /// Semantic role of the article in the collection.
    #[serde(default)]
    pub role: Option<String>,
    /// Explicit collection note. `None` preserves an existing note.
    #[serde(default)]
    pub note: Option<String>,
    /// How evidence and existing collection notes are resolved.
    #[serde(default)]
    pub note_policy: Option<CollectionNotePolicy>,
    /// Whether cached articles also join the collection.
    #[serde(default)]
    pub scope: Option<CollectionScope>,
}

/// Spec for [`BibSaveNode`].
#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
pub struct BibSaveSpec {
    /// Existing-library handling for each evidence record.
    #[serde(default)]
    pub on_existing: Option<ExistingPolicy>,
    /// Batch validation and transactional write semantics.
    #[serde(default)]
    pub write_mode: Option<WriteMode>,
    /// Require at least one stable identifier per record.
    #[serde(default)]
    pub require_identifier: Option<bool>,
    /// Best-effort Europe PMC OA fetch after a successful metadata ingest.
    #[serde(default)]
    pub fetch_fulltext: Option<bool>,
    /// Fetch for newly inserted records only, or for all records missing text.
    #[serde(default)]
    pub fulltext_scope: Option<CollectionScope>,
    /// Validate and report without writing to the library.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Maximum number of evidence records considered after deduplication.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Optional existing collection to receive imported articles.
    #[serde(default)]
    pub collection: Option<BibSaveCollectionSpec>,
}

fn report_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("input_index", DataType::UInt32, false),
        Field::new("record_key", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("article_id", DataType::Utf8, true),
        Field::new("title", DataType::Utf8, true),
        Field::new("doi", DataType::Utf8, true),
        Field::new("pmid", DataType::Utf8, true),
        Field::new("origin", DataType::Utf8, true),
        Field::new("fulltext_fetched", DataType::Boolean, false),
        Field::new("error", DataType::Utf8, true),
        Field::new("collection_id", DataType::Utf8, true),
        Field::new("collection_action", DataType::Utf8, true),
        Field::new("role", DataType::Utf8, true),
        Field::new("collection_note", DataType::Utf8, true),
        Field::new("collection_error", DataType::Utf8, true),
    ]))
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label_and_format(None, PortType::File, "evidence", FORMAT)
        .add_output_port(Some(report_schema()))
}

pub struct BibSaveNode {
    meta: NodePorts,
    spec: BibSaveSpec,
    shared: Option<Arc<BibShared>>,
}

impl BibSaveNode {
    pub fn new(spec: BibSaveSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
            shared: None,
        }
    }

    /// Build a node bound to a caller-supplied bibliography (tests).
    pub fn with_shared(spec: BibSaveSpec, shared: Arc<BibShared>) -> Self {
        Self {
            meta: port_layout(),
            spec,
            shared: Some(shared),
        }
    }

    fn shared(&self) -> Result<Arc<BibShared>, DagError> {
        self.shared.clone().map(Ok).unwrap_or_else(shared_bib)
    }
}

#[derive(Debug)]
struct ImportRow {
    input_index: u32,
    record_key: String,
    status: &'static str,
    article_id: Option<String>,
    title: Option<String>,
    doi: Option<String>,
    pmid: Option<String>,
    origin: Option<String>,
    fulltext_fetched: bool,
    error: Option<String>,
    collection_id: Option<String>,
    collection_action: Option<String>,
    role: Option<String>,
    collection_note: Option<String>,
    collection_error: Option<String>,
}

fn normalized_identifier(kind: IdKind, value: &str) -> String {
    if kind == IdKind::Doi {
        bib_types::convert::normalize_doi(value)
    } else {
        value.trim().to_owned()
    }
}

fn identifier_candidates(kind: IdKind, value: &str) -> Vec<String> {
    let primary = normalized_identifier(kind, value);
    let mut candidates = vec![primary.clone()];
    let lowercase = primary.trim().to_lowercase();
    if lowercase != primary {
        candidates.push(lowercase);
    }
    candidates
}

fn record_key(citation: &Article) -> String {
    for kind in [
        IdKind::Doi,
        IdKind::Pmid,
        IdKind::Pmc,
        IdKind::Arxiv,
        IdKind::OpenAlex,
        IdKind::S2,
        IdKind::Biorxiv,
    ] {
        if let Some(id) = citation.identifier(kind) {
            return format!("{}:{}", kind.as_str(), normalized_identifier(kind, id));
        }
    }
    if citation.id.trim().is_empty() {
        format!("fuzzy:{}", cite_key(citation))
    } else {
        citation.id.clone()
    }
}

fn action_name(outcome: &CollectionAddOutcome) -> &'static str {
    match outcome {
        CollectionAddOutcome::Inserted => "added",
        CollectionAddOutcome::Updated {
            role_changed,
            note_changed,
            ..
        } => {
            if *role_changed || *note_changed {
                "updated"
            } else {
                "unchanged"
            }
        }
    }
}

fn parse_collection_role(raw: &str) -> Result<ArticleRole, DagError> {
    match raw.trim().to_lowercase().as_str() {
        "requested" => Ok(ArticleRole::Requested),
        "referenced" => Ok(ArticleRole::Referenced),
        "cited" => Ok(ArticleRole::Cited),
        "background" => Ok(ArticleRole::Background),
        other => Err(DagError::Schedule(format!(
            "invalid collection role `{other}`; expected requested, referenced, cited, or background"
        ))),
    }
}

fn collection_note<'a>(
    collection: &'a BibSaveCollectionSpec,
    record: &'a bib_types::evidence::EvidenceRecord,
) -> Option<&'a str> {
    match collection.note_policy.unwrap_or_default() {
        CollectionNotePolicy::FromEvidence => record.note.as_deref(),
        CollectionNotePolicy::Preserve | CollectionNotePolicy::Overwrite => {
            collection.note.as_deref()
        }
    }
}

#[async_trait]
impl DagNode for BibSaveNode {
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
        "bib_save"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or_else(|| {
            DagError::Schedule("bib_save requires exactly one evidence input".into())
        })?;
        let file = input.file_value()?;
        if let Some(format) = file.format.as_deref()
            && format != FORMAT
        {
            return Err(DagError::Schedule(format!(
                "bib_save input carries format `{format}`, expected `{FORMAT}`"
            )));
        }

        let shared = self.shared()?;
        let bib = shared.bib.clone();
        let bytes = read_file_bytes(node_ctx, &file.path).await?;
        let mut set = EvidenceSet::parse(&bytes).map_err(|error| {
            DagError::Schedule(format!("bib_save input `{}`: {error}", file.path))
        })?;
        set.dedup_in_place();

        let on_existing = self.spec.on_existing.unwrap_or_default();
        let write_mode = self.spec.write_mode.unwrap_or_default();
        let require_identifier = self.spec.require_identifier.unwrap_or(true);
        let fetch_fulltext = self.spec.fetch_fulltext.unwrap_or(false);
        let fulltext_scope = self.spec.fulltext_scope.unwrap_or_default();
        let dry_run = self.spec.dry_run.unwrap_or(false);
        let parsed_collection_role = self
            .spec
            .collection
            .as_ref()
            .map(|collection| {
                collection
                    .role
                    .as_deref()
                    .map(parse_collection_role)
                    .unwrap_or_else(|| Ok(ArticleRole::Referenced))
            })
            .transpose()?;
        let limit = self
            .spec
            .limit
            .map(|limit| limit as usize)
            .unwrap_or(set.records.len());

        if let Some(collection) = &self.spec.collection {
            let exists = bib.get_collection(&collection.id).await.map_err(|error| {
                DagError::Schedule(format!("bib_save collection lookup: {error}"))
            })?;
            if exists.is_none() {
                return Err(DagError::Schedule(format!(
                    "bib_save collection `{}` does not exist",
                    collection.id
                )));
            }
        }

        let mut rows: Vec<ImportRow> = Vec::with_capacity(set.records.len());
        let mut articles_to_write: Vec<Article> = Vec::new();
        let mut assignments: Vec<CollectionAssignment> = Vec::new();

        for (input_index, record) in set.records.iter().enumerate() {
            if input_index >= limit {
                continue;
            }
            let citation = &record.citation;
            let mut row = ImportRow {
                input_index: input_index as u32,
                record_key: record_key(citation),
                status: "saved",
                article_id: None,
                title: Some(citation.title.clone()),
                doi: citation.doi().map(str::to_owned),
                pmid: citation.pmid().map(str::to_owned),
                origin: record.origin.clone(),
                fulltext_fetched: false,
                error: None,
                collection_id: None,
                collection_action: None,
                role: None,
                collection_note: None,
                collection_error: None,
            };

            if citation.title.trim().is_empty() {
                row.status = "failed";
                row.error = Some("article title is required".into());
                rows.push(row);
                continue;
            }

            let mut citation = citation.clone();
            for identifier in &mut citation.identifiers {
                identifier.value = normalized_identifier(identifier.kind, &identifier.value);
            }
            let identifiers = citation
                .identifiers
                .iter()
                .map(|identifier| (identifier.kind, identifier.value.as_str()))
                .collect::<Vec<_>>();
            if identifiers.is_empty() {
                if require_identifier {
                    row.status = "failed";
                    row.error = Some("at least one article identifier is required".into());
                    rows.push(row);
                    continue;
                }
                citation.id = format!("fuzzy:{}", cite_key(&citation));
            }

            let mut existing_id = None;
            for (kind, value) in identifiers {
                for candidate in identifier_candidates(kind, value) {
                    if let Some(existing) =
                        bib.find_by_identifier(kind, &candidate)
                            .await
                            .map_err(|error| {
                                DagError::Schedule(format!("bib_save library lookup: {error}"))
                            })?
                    {
                        existing_id = Some(existing.id);
                        break;
                    }
                }
                if existing_id.is_some() {
                    break;
                }
            }
            if existing_id.is_none() {
                if let Some(existing) = bib.get_article(&citation.id).await.map_err(|error| {
                    DagError::Schedule(format!("bib_save article lookup: {error}"))
                })? {
                    existing_id = Some(existing.id);
                }
            }

            let Some(existing_id) = existing_id else {
                articles_to_write.push(citation.clone());
                row.article_id = Some(citation.id.clone());
                if let Some(collection) = &self.spec.collection {
                    row.collection_id = Some(collection.id.clone());
                    row.role = Some(
                        parsed_collection_role
                            .unwrap_or_default()
                            .as_str()
                            .to_owned(),
                    );
                }
                if dry_run {
                    row.status = "would_save";
                    row.collection_action = self
                        .spec
                        .collection
                        .as_ref()
                        .map(|_| "would_add".to_owned());
                } else {
                    if let Some(collection) = &self.spec.collection {
                        assignments.push(CollectionAssignment {
                            article_id: citation.id.clone(),
                            role: parsed_collection_role.unwrap_or_default(),
                            added_by: AddedBy::Agent,
                            note: collection_note(collection, record).map(str::to_owned),
                        });
                    }
                }
                rows.push(row);
                continue;
            };

            row.article_id = Some(existing_id.clone());
            match on_existing {
                ExistingPolicy::Fail => {
                    row.status = "failed";
                    row.error = Some(format!("article already exists as `{existing_id}`"));
                }
                ExistingPolicy::Skip => {
                    row.status = "cached";
                    if let Some(collection) = &self.spec.collection {
                        row.collection_id = Some(collection.id.clone());
                        row.role = Some(
                            parsed_collection_role
                                .unwrap_or_default()
                                .as_str()
                                .to_owned(),
                        );
                        row.collection_action = Some("would_add".to_owned());
                        if !dry_run
                            && collection.scope.unwrap_or_default() == CollectionScope::AllMatched
                        {
                            assignments.push(CollectionAssignment {
                                article_id: existing_id.clone(),
                                role: parsed_collection_role.unwrap_or_default(),
                                added_by: AddedBy::Agent,
                                note: collection_note(collection, record).map(str::to_owned),
                            });
                        }
                    }
                }
                ExistingPolicy::Refresh => {
                    citation.id = existing_id.clone();
                    articles_to_write.push(citation.clone());
                    row.status = if dry_run { "would_save" } else { "saved" };
                    if let Some(collection) = &self.spec.collection {
                        row.collection_id = Some(collection.id.clone());
                        row.role = Some(
                            parsed_collection_role
                                .unwrap_or_default()
                                .as_str()
                                .to_owned(),
                        );
                        row.collection_action = Some("would_add".to_owned());
                        if !dry_run
                            && collection.scope.unwrap_or_default() == CollectionScope::AllMatched
                        {
                            assignments.push(CollectionAssignment {
                                article_id: existing_id.clone(),
                                role: parsed_collection_role.unwrap_or_default(),
                                added_by: AddedBy::Agent,
                                note: collection_note(collection, record).map(str::to_owned),
                            });
                        }
                    }
                }
            }
            rows.push(row);
        }

        for (input_index, record) in set.records.iter().enumerate().skip(limit) {
            rows.push(ImportRow {
                input_index: input_index as u32,
                record_key: record_key(&record.citation),
                status: "skipped_limit",
                article_id: None,
                title: Some(record.citation.title.clone()),
                doi: record.citation.doi().map(str::to_owned),
                pmid: record.citation.pmid().map(str::to_owned),
                origin: record.origin.clone(),
                fulltext_fetched: false,
                error: Some(format!("limit is {limit}")),
                collection_id: None,
                collection_action: None,
                role: None,
                collection_note: None,
                collection_error: None,
            });
        }

        if write_mode == WriteMode::Atomic {
            let errors: Vec<String> = rows
                .iter()
                .filter(|row| row.status == "failed")
                .filter_map(|row| row.error.clone())
                .collect();
            if !errors.is_empty() {
                return Err(DagError::Schedule(format!(
                    "bib_save atomic validation failed: {}",
                    errors.join("; ")
                )));
            }
        }

        if !dry_run {
            let collection = self.spec.collection.as_ref();
            let collection_id = collection.map(|collection| collection.id.as_str());
            let note_policy = collection
                .and_then(|collection| collection.note_policy)
                .unwrap_or_default();
            let articles = articles_to_write.clone();
            let mut tx_assignments = Vec::with_capacity(assignments.len());
            std::mem::swap(&mut tx_assignments, &mut assignments);

            let outcomes = bib
                .upsert_articles_with_collection(
                    &articles,
                    collection_id.unwrap_or(""),
                    &tx_assignments,
                )
                .await
                .map_err(|error| DagError::Schedule(format!("bib_save ingest failed: {error}")))?;
            let mut outcome_by_article = outcomes
                .into_iter()
                .map(|outcome| (outcome.article_id, outcome.outcome))
                .collect::<std::collections::HashMap<_, _>>();

            for row in &mut rows {
                let Some(article_id) = row.article_id.clone() else {
                    continue;
                };
                if collection_id.is_none() {
                    continue;
                }
                let collection = collection.unwrap();
                let scope = collection.scope.unwrap_or_default();
                if scope == CollectionScope::NewOnly && row.status != "saved" {
                    row.collection_action = Some("skipped".to_owned());
                    continue;
                }
                if let Some(outcome) = outcome_by_article.remove(&article_id) {
                    let note = match note_policy {
                        CollectionNotePolicy::FromEvidence => set
                            .records
                            .get(row.input_index as usize)
                            .and_then(|record| record.note.as_deref()),
                        _ => collection.note.as_deref(),
                    };
                    row.collection_action = Some(action_name(&outcome).to_owned());
                    row.collection_note = note.map(str::to_owned);
                } else if scope == CollectionScope::NewOnly {
                    row.collection_action = Some("skipped".to_owned());
                }
            }

            if fetch_fulltext {
                let Some(storage) = shared.file_storage.as_ref() else {
                    return Err(DagError::Schedule(
                        "bib_save fetch_fulltext requires mounted VFS file storage".into(),
                    ));
                };
                for article in &articles {
                    let should_fetch = fulltext_scope == CollectionScope::AllMatched
                        || rows.iter().any(|row| {
                            row.article_id.as_deref() == Some(article.id.as_str())
                                && row.status == "saved"
                        });
                    if !should_fetch {
                        continue;
                    }
                    match fetch_fulltext_stored(&shared.europe_pmc, storage, article).await {
                        Some(fulltext) => {
                            bib.upsert_fulltext(&fulltext).await.map_err(|error| {
                                DagError::Schedule(format!(
                                    "bib_save stored fetched full text for `{}`: {error}",
                                    article.id
                                ))
                            })?;
                            if let Some(row) = rows
                                .iter_mut()
                                .find(|row| row.article_id.as_deref() == Some(article.id.as_str()))
                            {
                                row.fulltext_fetched = true;
                            }
                        }
                        None => reporter.warn(format!(
                            "no Europe PMC open-access full text found for article `{}`",
                            article.id
                        )),
                    }
                }
            }
        }

        let record_count = rows.len();
        let to_option = |value: &Option<String>| value.as_deref().map(str::to_owned);
        let batch = RecordBatch::try_new(
            report_schema(),
            vec![
                Arc::new(UInt32Array::from(
                    rows.iter().map(|row| row.input_index).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| row.record_key.clone())
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter().map(|row| row.status).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.article_id))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.title))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.doi))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.pmid))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.origin))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(BooleanArray::from(
                    rows.iter()
                        .map(|row| row.fulltext_fetched)
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.error))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.collection_id))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.collection_action))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.role))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.collection_note))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter()
                        .map(|row| to_option(&row.collection_error))
                        .collect::<Vec<_>>(),
                )),
            ],
        )
        .map_err(|error| DagError::Schedule(format!("bib_save report batch: {error}")))?;

        reporter.info(format!(
            "bib_save processed {record_count} evidence record(s)"
        ));
        let mut outputs = PortOutputs::new();
        outputs.insert(
            0,
            node_ctx.session().read_batch(batch).map_err(|error| {
                DagError::Schedule(format!("bib_save report dataframe: {error}"))
            })?,
        );
        Ok(outputs)
    }
}

pub struct BibSaveNodeFactory {}

#[async_trait]
impl NodeFactory for BibSaveNodeFactory {
    fn kind(&self) -> &'static str {
        "bib_save"
    }

    fn desc(&self) -> &'static str {
        "Import evidence citations into the bibliography library"
    }

    fn doc(&self) -> &'static str {
        "Takes one `evidence` artifact, matches each canonical citation against \
         the shared bibliography by any identifier, and atomically saves new or \
         refreshed metadata. Optionally attaches matched articles to an existing \
         collection with a semantic role and optional note.\n\
         \n\
         The output is a DataFrame audit report with one row per evidence \
         record (`status`, `article_id`, collection action, and error details). \
         Use `dry_run: true` for a no-write preview. Full-text fetching is off \
         by default; wire `literature_fulltext` for full-text resolution."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(BibSaveSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: BibSaveSpec = serde_json::from_value(spec)?;
        Ok(Box::new(BibSaveNode::new(spec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Array, StringArray};
    use bib_types::evidence::EvidenceRecord;
    use bib_types::types::{Collection, Identifier};
    use dag_core::dag::node_event::NodeReporter;
    use dag_core::value::FileRef;

    fn node_ctx_with_evidence_storage() -> (NodeCtx, Arc<vfs::OpendalFileStorage>) {
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage.clone()),
        );
        (ctx, storage)
    }

    async fn shared_with_collection() -> Arc<BibShared> {
        let shared = BibShared::open_in_memory()
            .await
            .unwrap()
            .with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp()));
        shared
            .bib
            .upsert_collection(&Collection::new("coll-1", "Test collection"))
            .await
            .unwrap();
        Arc::new(shared)
    }

    async fn write_evidence(
        storage: &vfs::OpendalFileStorage,
        records: Vec<EvidenceRecord>,
    ) -> String {
        let path = format!("/tmp/bib-save-node-{}.json", uuid::Uuid::new_v4());
        let set = EvidenceSet {
            records,
            ..Default::default()
        };
        storage
            .op
            .write(&path, set.to_bytes().unwrap())
            .await
            .unwrap();
        path
    }

    fn spec() -> BibSaveSpec {
        BibSaveSpec {
            collection: Some(BibSaveCollectionSpec {
                id: "coll-1".into(),
                role: Some("background".into()),
                note: None,
                note_policy: None,
                scope: None,
            }),
            ..Default::default()
        }
    }

    async fn column_values(outputs: &PortOutputs, column: &str) -> Vec<Option<String>> {
        let batches = outputs
            .dataframe(0)
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        batches
            .iter()
            .flat_map(|batch| {
                let values = batch
                    .column_by_name(column)
                    .unwrap()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                (0..values.len())
                    .map(|index| {
                        if values.is_null(index) {
                            None
                        } else {
                            Some(values.value(index).to_owned())
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[tokio::test]
    async fn imports_evidence_and_attaches_collection_atomically() {
        let shared = shared_with_collection().await;
        let mut existing = Article::new("existing-1", "Existing study");
        existing.identifiers.push(Identifier::doi("10.1/existing"));
        shared.bib.upsert_article(&existing).await.unwrap();

        let mut existing_citation = Article::new("evidence-existing", "Existing study");
        existing_citation
            .identifiers
            .push(Identifier::doi("https://doi.org/10.1/Existing"));
        let mut new_citation = Article::new("evidence-new", "New study");
        new_citation.identifiers.push(Identifier::doi("10.1/new"));

        let (ctx, storage) = node_ctx_with_evidence_storage();
        let path = write_evidence(
            &storage,
            vec![
                EvidenceRecord {
                    citation: existing_citation,
                    note: Some("already known".into()),
                    origin: Some("pubmed".into()),
                },
                EvidenceRecord {
                    citation: new_citation,
                    note: Some("new evidence".into()),
                    origin: Some("crossref".into()),
                },
            ],
        )
        .await;

        let inputs = vec![NodeInput::file(0, FileRef::new(&path, Some(FORMAT.into())))];
        let mut dry = BibSaveNode::with_shared(
            BibSaveSpec {
                dry_run: Some(true),
                ..spec()
            },
            shared.clone(),
        );
        let dry_outputs = dry
            .execute(&ctx, &inputs, &NodeReporter::noop())
            .await
            .unwrap();
        assert_eq!(
            column_values(&dry_outputs, "status").await,
            vec![Some("cached".into()), Some("would_save".into())]
        );
        assert_eq!(
            shared.bib.list_collections(None).await.unwrap()[0]
                .article_ids
                .len(),
            0
        );

        let mut node = BibSaveNode::with_shared(spec(), shared.clone());
        let outputs = node
            .execute(&ctx, &inputs, &NodeReporter::noop())
            .await
            .unwrap();
        assert_eq!(
            column_values(&outputs, "status").await,
            vec![Some("cached".into()), Some("saved".into())]
        );
        assert_eq!(
            column_values(&outputs, "collection_action").await,
            vec![Some("added".into()), Some("added".into())]
        );

        let collection = shared.bib.get_collection("coll-1").await.unwrap().unwrap();
        assert_eq!(collection.article_ids, vec!["existing-1", "evidence-new"]);
    }

    #[tokio::test]
    async fn second_run_is_idempotent() {
        let shared = shared_with_collection().await;
        let mut citation = Article::new("stable-id", "Stable study");
        citation.identifiers.push(Identifier::doi("10.1/stable"));

        let (ctx, storage) = node_ctx_with_evidence_storage();
        let path = write_evidence(
            &storage,
            vec![EvidenceRecord {
                citation,
                note: None,
                origin: None,
            }],
        )
        .await;
        let inputs = vec![NodeInput::file(0, FileRef::new(&path, Some(FORMAT.into())))];
        let mut node = BibSaveNode::with_shared(spec(), shared.clone());
        node.execute(&ctx, &inputs, &NodeReporter::noop())
            .await
            .unwrap();

        let mut replay = BibSaveNode::with_shared(spec(), shared.clone());
        let outputs = replay
            .execute(&ctx, &inputs, &NodeReporter::noop())
            .await
            .unwrap();
        assert_eq!(
            column_values(&outputs, "status").await,
            vec![Some("cached".into())]
        );
        assert_eq!(
            column_values(&outputs, "collection_action").await,
            vec![Some("unchanged".into())]
        );
    }
}
