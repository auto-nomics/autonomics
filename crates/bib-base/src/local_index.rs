//! Versioned Tantivy projection. SQLite is authoritative; article revisions
//! can be replayed after a failed or interrupted index commit.
use crate::{
    Error, Result,
    bib_base::{SearchCandidate, tokenize, unique_query_terms},
};
use bib_types::{SearchExpr, SearchField};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
use tantivy::{
    DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Searcher, TantivyDocument, Term,
    collector::TopDocs,
    index::SegmentId,
    query::{AllQuery, BooleanQuery, BoostQuery, Occur, PhraseQuery, Query, TermQuery},
    schema::{
        Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions,
        Value as _,
    },
    tokenizer::TextAnalyzer,
};

type FastMap<K, V> = HashMap<K, V, ahash::RandomState>;

mod scored_docs;
use scored_docs::AllScoredDocs;

pub(crate) const TEXT_FIELDS: [&str; 10] = [
    "title",
    "abstract",
    "creator",
    "publication",
    "identifier",
    "keyword",
    "tag",
    "year_text",
    "annotation",
    "fulltext",
];

pub(crate) struct SourceDocument {
    pub meta: SearchCandidate,
    pub texts: [String; 10],
    pub identifiers: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct IndexProgress {
    pub revision: Option<u64>,
    pub documents: usize,
    pub durable: bool,
    pub last_error: Option<String>,
    pub building: bool,
    pub processed_documents: usize,
    pub target_revision: Option<u64>,
}

pub(crate) struct LocalIndex {
    pub(crate) filter_cache: crate::search_filters::MetadataCache,
    pub progress: std::sync::Arc<std::sync::Mutex<IndexProgress>>,
    reader: IndexReader,
    writer: IndexWriter,
    fields: HashMap<&'static str, Field>,
    metadata: FastMap<String, SearchCandidate>,
    doc_ids: FastMap<SegmentId, Arc<Vec<String>>>,
    pub revision: Option<u64>,
    pub commit_token: Option<String>,
    pub source_checked: bool,
    pending: Option<PendingUpdate>,
    #[cfg(test)]
    pub fail_next_commit: bool,
    #[cfg(test)]
    pub pause_before_commit: Option<(
        tokio::sync::oneshot::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
}
struct PendingUpdate {
    articles: HashSet<String>,
    metadata: Vec<SearchCandidate>,
    revision: u64,
    commit_token: String,
    rebuild: bool,
}

fn schema() -> (Schema, HashMap<&'static str, Field>) {
    let mut builder = Schema::builder();
    let mut fields = HashMap::new();
    for name in [
        "doc_id",
        "article_id",
        "anchor_key",
        "kind",
        "display_title",
        "year_value",
    ] {
        fields.insert(name, builder.add_text_field(name, STRING | STORED));
    }
    fields.insert(
        "identifier_exact",
        builder.add_text_field("identifier_exact", STRING),
    );
    let options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("bibliography-v2")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    for name in TEXT_FIELDS {
        fields.insert(name, builder.add_text_field(name, options.clone()));
    }
    (builder.build(), fields)
}

impl LocalIndex {
    pub fn open(database: &str) -> Result<Self> {
        let (schema, fields) = schema();
        let mut durable = database != ":memory:";
        let mut index = if durable {
            let path = PathBuf::from(format!("{database}.search-v2"));
            std::fs::create_dir_all(&path)?;
            if path.join("meta.json").exists() {
                Index::open_in_dir(&path)?
            } else {
                Index::create_in_dir(&path, schema.clone())?
            }
        } else {
            Index::create_in_ram(schema.clone())
        };
        if index.schema() != schema {
            return Err(Error::Unknown(
                "search index schema differs from v2; rebuild using a new generation".into(),
            ));
        }
        let writer = match index.writer_with_num_threads(1, 20_000_000) {
            Ok(writer) => writer,
            Err(tantivy::TantivyError::LockFailure(_, _)) if durable => {
                // A second process keeps its own replay cursor and RAM index.
                // The latest event per article is retained in SQLite, so it
                // cannot miss changes acknowledged by another process.
                tracing::info!(
                    "search index writer owned by another process; using a local RAM projection"
                );
                durable = false;
                index = Index::create_in_ram(schema);
                index.writer_with_num_threads(1, 20_000_000)?
            }
            Err(error) => return Err(error.into()),
        };
        index.tokenizers().register(
            "bibliography-v2",
            TextAnalyzer::from(crate::search_analyzer::BibliographyTokenizer),
        );
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        let payload = index.load_metas()?.payload;
        let commit = payload
            .as_deref()
            .and_then(|value| serde_json::from_str::<(u64, String)>(value).ok());
        let revision = commit.as_ref().map(|(revision, _)| *revision).or_else(|| {
            payload
                .as_deref()
                .and_then(|value| value.strip_prefix("bibliography-v2:"))
                .and_then(|value| value.parse().ok())
        });
        let commit_token = commit.map(|(_, token)| token);
        let progress = std::sync::Arc::new(std::sync::Mutex::new(IndexProgress {
            revision,
            documents: 0,
            durable,
            last_error: None,
            building: false,
            processed_documents: 0,
            target_revision: None,
        }));
        let mut local = Self {
            progress,
            reader,
            writer,
            fields,
            metadata: FastMap::default(),
            doc_ids: FastMap::default(),
            filter_cache: Default::default(),
            revision,
            commit_token,
            source_checked: false,
            pending: None,
            #[cfg(test)]
            fail_next_commit: false,
            #[cfg(test)]
            pause_before_commit: None,
        };
        let searcher = local.reader.searcher();
        let mut doc_ids = searcher
            .segment_readers()
            .iter()
            .map(|segment| {
                (
                    segment.segment_id(),
                    vec![String::new(); segment.max_doc() as usize],
                )
            })
            .collect::<HashMap<_, _>>();
        if searcher.num_docs() > 0 {
            for (_, address) in searcher.search(
                &AllQuery,
                &TopDocs::with_limit(searcher.num_docs() as usize).order_by_score(),
            )? {
                let stored: TantivyDocument = searcher.doc(address)?;
                let get = |name| {
                    stored
                        .get_first(local.fields[name])
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_owned()
                };
                let candidate = SearchCandidate {
                    doc_id: get("doc_id"),
                    article_id: get("article_id"),
                    anchor_key: get("anchor_key"),
                    kind: get("kind"),
                    title: get("display_title"),
                    year: get("year_value").parse().ok(),
                    score: 0.0,
                    matched_field: "title".into(),
                    snippet_query: String::new(),
                };
                doc_ids
                    .get_mut(&searcher.segment_reader(address.segment_ord).segment_id())
                    .expect("reader segment")[address.doc_id as usize] = candidate.doc_id.clone();
                local.metadata.insert(candidate.doc_id.clone(), candidate);
            }
        }
        local.doc_ids = doc_ids
            .into_iter()
            .map(|(id, ids)| (id, Arc::new(ids)))
            .collect();
        local.progress.lock().expect("new progress lock").documents = local.metadata.len();
        Ok(local)
    }

    /// Segment identifiers and document ordinals remain stable until a merge.
    /// Reuse unchanged segments; read each new stored identity only once per
    /// reader generation, rather than once for every term/field in each query.
    fn refresh_doc_ids(&mut self) -> Result<()> {
        let searcher = self.reader.searcher();
        let mut next = FastMap::default();
        for (ordinal, segment) in searcher.segment_readers().iter().enumerate() {
            let id = segment.segment_id();
            let ids = match self.doc_ids.get(&id) {
                Some(ids) => ids.clone(),
                None => {
                    let mut ids = vec![String::new(); segment.max_doc() as usize];
                    for doc in 0..segment.max_doc() {
                        if segment.is_deleted(doc) {
                            continue;
                        }
                        let stored: TantivyDocument = searcher.doc(DocAddress {
                            segment_ord: ordinal as u32,
                            doc_id: doc,
                        })?;
                        ids[doc as usize] = stored
                            .get_first(self.fields["doc_id"])
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_owned();
                    }
                    Arc::new(ids)
                }
            };
            next.insert(id, ids);
        }
        self.doc_ids = next;
        Ok(())
    }

    pub fn begin_update(
        &mut self,
        articles: Vec<String>,
        revision: u64,
        commit_token: String,
        rebuild: bool,
    ) -> Result<()> {
        if self.pending.is_some() {
            self.abort_update()?;
        }
        if rebuild {
            self.writer.delete_all_documents()?;
        } else {
            for article in &articles {
                self.writer
                    .delete_term(Term::from_field_text(self.fields["article_id"], article));
            }
        }
        if let Ok(mut progress) = self.progress.lock() {
            progress.building = true;
            progress.processed_documents = 0;
            progress.target_revision = Some(revision);
        }
        self.pending = Some(PendingUpdate {
            articles: articles.into_iter().collect(),
            metadata: Vec::new(),
            revision,
            commit_token,
            rebuild,
        });
        Ok(())
    }

    pub fn append_documents(&mut self, documents: Vec<SourceDocument>) -> Result<()> {
        if self.pending.is_none() {
            return Err(Error::Unknown("index update has not started".into()));
        }
        let appended = documents.len();
        for source in documents {
            let mut doc = TantivyDocument::default();
            for (name, value) in [
                ("doc_id", source.meta.doc_id.as_str()),
                ("article_id", &source.meta.article_id),
                ("anchor_key", &source.meta.anchor_key),
                ("kind", &source.meta.kind),
                ("display_title", &source.meta.title),
            ] {
                doc.add_text(self.fields[name], value);
            }
            if let Some(year) = source.meta.year {
                doc.add_text(self.fields["year_value"], year.to_string());
            }
            for (field, text) in TEXT_FIELDS.iter().zip(&source.texts) {
                if *field == "keyword" {
                    let values = serde_json::from_str::<Vec<String>>(text)
                        .unwrap_or_else(|_| vec![text.clone()]);
                    doc.add_text(self.fields[field], values.join("\u{1e}"));
                } else {
                    doc.add_text(self.fields[field], text);
                }
            }
            for identifier in &source.identifiers {
                doc.add_text(
                    self.fields["identifier_exact"],
                    normalize_identifier(identifier),
                );
            }
            self.writer.add_document(doc)?;
            self.pending
                .as_mut()
                .expect("checked pending update")
                .metadata
                .push(source.meta);
        }
        if let Ok(mut progress) = self.progress.lock() {
            progress.processed_documents += appended;
        }
        Ok(())
    }

    pub fn finish_update(&mut self) -> Result<()> {
        let pending = self
            .pending
            .take()
            .ok_or_else(|| Error::Unknown("index update has not started".into()))?;
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_commit) {
            return Err(Error::Unknown("injected index commit failure".into()));
        }
        #[cfg(test)]
        if let Some((entered, release)) = self.pause_before_commit.take() {
            let _ = entered.send(());
            let _ = release.recv_timeout(std::time::Duration::from_secs(5));
        }
        let mut commit = self.writer.prepare_commit()?;
        commit.set_payload(&serde_json::to_string(&(
            pending.revision,
            pending.commit_token.as_str(),
        ))?);
        commit.commit()?;
        self.reader.reload()?;
        self.refresh_doc_ids()?;
        if pending.rebuild {
            self.metadata.clear();
        } else {
            self.metadata
                .retain(|_, doc| !pending.articles.contains(&doc.article_id));
        }
        for meta in pending.metadata {
            self.metadata.insert(meta.doc_id.clone(), meta);
        }
        self.revision = Some(pending.revision);
        self.commit_token = Some(pending.commit_token);
        self.source_checked = true;
        if let Ok(mut progress) = self.progress.lock() {
            progress.revision = Some(pending.revision);
            progress.documents = self.metadata.len();
            progress.last_error = None;
            progress.building = false;
            progress.target_revision = None;
        }
        Ok(())
    }

    pub fn abort_update(&mut self) -> Result<()> {
        self.pending = None;
        self.writer.rollback()?;
        if let Ok(mut progress) = self.progress.lock() {
            progress.building = false;
            progress.target_revision = None;
        }
        Ok(())
    }

    fn universe(&self, same_child: bool) -> Matches<'_> {
        let mut out: Matches<'_> = FastMap::default();
        for candidate in self.metadata.values() {
            let key = if same_child {
                &candidate.doc_id
            } else {
                &candidate.article_id
            };
            let replace = out
                .get(key.as_str())
                .is_none_or(|old: &Match| evidence_order(candidate, &old.candidate).is_lt());
            if replace {
                out.insert(
                    key.as_str(),
                    Match {
                        candidate: CandidateRef::new(candidate),
                        best_score: 0.0,
                    },
                );
            }
        }
        out
    }

    pub fn search(
        &self,
        expression: Option<&SearchExpr>,
        same_child: bool,
    ) -> Result<Vec<SearchCandidate>> {
        self.search_in(expression, same_child, None)
    }

    pub(crate) fn search_in(
        &self,
        expression: Option<&SearchExpr>,
        same_child: bool,
        allowed: Option<&HashSet<String>>,
    ) -> Result<Vec<SearchCandidate>> {
        self.search_inner(expression, same_child, allowed, None)
    }

    pub(crate) fn search_top_in(
        &self,
        expression: &SearchExpr,
        allowed: &HashSet<String>,
        limit: usize,
    ) -> Result<Vec<SearchCandidate>> {
        self.search_inner(Some(expression), false, Some(allowed), Some(limit))
    }

    fn search_inner(
        &self,
        expression: Option<&SearchExpr>,
        same_child: bool,
        allowed: Option<&HashSet<String>>,
        limit: Option<usize>,
    ) -> Result<Vec<SearchCandidate>> {
        if let Some(expr) = expression {
            validate(expr, 0, &mut 0)?;
        }
        // One coverage check avoids repeated membership hashing when the hard
        // universe contains every indexed article. A partial set still filters
        // each leaf before aggregation; permissions are never inferred by size.
        let allowed =
            allowed.filter(|set| !self.metadata.values().all(|m| set.contains(&m.article_id)));
        let searcher = self.reader.searcher();
        let mut matches = match expression {
            Some(expr) => self.evaluate_in(expr, same_child, &searcher, allowed)?,
            None => self.universe(false),
        };
        if let Some(allowed) = allowed {
            matches.retain(|_, hit| allowed.contains(&hit.candidate.article_id));
        }
        let mut articles: Matches<'_> = FastMap::default();
        for hit in matches.into_values() {
            let key = hit.candidate.meta.article_id.as_str();
            if let Some(previous) = articles.get(&key) {
                if hit.candidate.score < previous.candidate.score
                    || (hit.candidate.score == previous.candidate.score
                        && evidence_order(&hit.candidate, &previous.candidate).is_ge())
                {
                    continue;
                }
            }
            articles.insert(key, hit);
        }
        let mut ranked = articles
            .into_values()
            .map(|hit| hit.candidate)
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.article_id.cmp(&b.article_id))
                .then_with(|| a.doc_id.cmp(&b.doc_id))
        });
        if let Some(limit) = limit {
            ranked.truncate(limit);
        }
        Ok(ranked.into_iter().map(CandidateRef::into_owned).collect())
    }

    /// Collect actual positive-clause evidence only for an eligible article.
    /// OR visits successful branches; NOT contributes no positive snippet.
    pub(crate) fn evidence(
        &self,
        expression: Option<&SearchExpr>,
        same_child: bool,
        article_id: &str,
    ) -> Result<Vec<SearchCandidate>> {
        let searcher = self.reader.searcher();
        let allowed = if same_child {
            match expression {
                Some(expr) => self.evaluate(expr, true, &searcher)?,
                None => self.universe(true),
            }
        } else {
            Matches::default()
        };
        let mut out: HashMap<String, SearchCandidate> = HashMap::new();
        if let Some(expr) = expression {
            self.collect_evidence(
                expr,
                article_id,
                &searcher,
                same_child.then_some(&allowed),
                true,
                &mut out,
            )?;
        }
        let mut out = out.into_values().collect::<Vec<_>>();
        out.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.doc_id.cmp(&b.doc_id))
        });
        Ok(out)
    }
    fn collect_evidence(
        &self,
        expr: &SearchExpr,
        article_id: &str,
        searcher: &Searcher,
        allowed: Option<&Matches<'_>>,
        positive: bool,
        out: &mut HashMap<String, SearchCandidate>,
    ) -> Result<()> {
        let eligible = self.evaluate(expr, allowed.is_some() && positive, searcher)?;
        let matched = eligible
            .values()
            .any(|hit| hit.candidate.article_id == article_id);
        if matched != positive {
            return Ok(());
        }
        let narrowed = if positive {
            allowed.map(|allowed| {
                eligible
                    .iter()
                    .filter(|(key, _)| allowed.contains_key(*key))
                    .map(|(key, hit)| (*key, hit.clone()))
                    .collect::<Matches>()
            })
        } else {
            None
        };
        let allowed = if positive { narrowed.as_ref() } else { allowed };
        match expr {
            SearchExpr::And { clauses } | SearchExpr::Or { clauses } => {
                for clause in clauses {
                    self.collect_evidence(clause, article_id, searcher, allowed, positive, out)?;
                }
            }
            SearchExpr::Not { clause } => {
                self.collect_evidence(clause, article_id, searcher, allowed, !positive, out)?;
            }
            SearchExpr::Term { .. } | SearchExpr::Phrase { .. } | SearchExpr::Range { .. }
                if positive =>
            {
                for hit in self.evaluate(expr, true, searcher)?.into_values() {
                    let candidate = hit.candidate;
                    if candidate.article_id != article_id
                        || allowed.is_some_and(|allowed| {
                            !allowed.contains_key(candidate.meta.doc_id.as_str())
                        })
                    {
                        continue;
                    }
                    let mut candidate = candidate.into_owned();
                    if !matches!(candidate.matched_field.as_str(), "fulltext" | "annotation") {
                        if let Some(parent) = self
                            .metadata
                            .values()
                            .find(|m| m.article_id == article_id && m.kind == "item")
                        {
                            candidate.doc_id = parent.doc_id.clone();
                            candidate.anchor_key = parent.anchor_key.clone();
                            candidate.kind = "item".into();
                        }
                    }
                    if out
                        .get(&candidate.doc_id)
                        .is_none_or(|old| old.score < candidate.score)
                    {
                        out.insert(candidate.doc_id.clone(), candidate);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn evaluate(
        &self,
        expr: &SearchExpr,
        same_child: bool,
        searcher: &Searcher,
    ) -> Result<Matches<'_>> {
        self.evaluate_in(expr, same_child, searcher, None)
    }

    fn evaluate_in(
        &self,
        expr: &SearchExpr,
        same_child: bool,
        searcher: &Searcher,
        allowed: Option<&HashSet<String>>,
    ) -> Result<Matches<'_>> {
        match expr {
            SearchExpr::And { clauses } | SearchExpr::Or { clauses } => {
                let mut children = clauses.iter();
                let mut out = self.evaluate_in(
                    children.next().expect("validated nonempty group"),
                    same_child,
                    searcher,
                    allowed,
                )?;
                for clause in children {
                    let mut next = self.evaluate_in(clause, same_child, searcher, allowed)?;
                    if matches!(expr, SearchExpr::And { .. }) {
                        out.retain(|key, previous| {
                            if let Some(hit) = next.remove(key) {
                                merge(previous, hit);
                                true
                            } else {
                                false
                            }
                        });
                    } else {
                        for (key, hit) in next {
                            if let Some(previous) = out.get_mut(&key) {
                                merge(previous, hit);
                            } else {
                                out.insert(key, hit);
                            }
                        }
                    }
                }
                Ok(out)
            }
            SearchExpr::Not { clause } => {
                let excluded = self.evaluate_in(clause, false, searcher, allowed)?;
                let mut out = self.universe(same_child);
                out.retain(|_, hit| !excluded.contains_key(hit.candidate.meta.article_id.as_str()));
                Ok(out)
            }
            SearchExpr::Range { gte, lte, .. } => {
                let lower = gte.as_deref().map(parse_year).transpose()?;
                let upper = lte.as_deref().map(parse_year).transpose()?;
                let mut out = self.universe(same_child);
                out.retain(|_, hit| {
                    hit.candidate.year.is_some_and(|year| {
                        lower.is_none_or(|bound| year >= bound)
                            && upper.is_none_or(|bound| year <= bound)
                    })
                });
                for hit in out.values_mut() {
                    hit.candidate.matched_field = "year_text".into();
                    hit.candidate.snippet_query = hit
                        .candidate
                        .year
                        .map(|year| year.to_string())
                        .unwrap_or_default()
                        .into();
                }
                Ok(out)
            }
            SearchExpr::Term { value, field } | SearchExpr::Phrase { value, field } => {
                let field = field.unwrap_or(SearchField::Any);
                if field == SearchField::Year {
                    let year = parse_year(value)?;
                    let mut out = self.universe(same_child);
                    out.retain(|_, hit| hit.candidate.year == Some(year));
                    for hit in out.values_mut() {
                        hit.candidate.matched_field = "year_text".into();
                        hit.candidate.snippet_query = value.as_str().into();
                    }
                    return Ok(out);
                }
                let docs = if field == SearchField::Identifier {
                    let normalized = normalize_identifier(value);
                    let query = TermQuery::new(
                        Term::from_field_text(self.fields["identifier_exact"], &normalized),
                        IndexRecordOption::Basic,
                    );
                    let mut matches =
                        self.run_query(&query, "identifier", value, 20.0, searcher, allowed)?;
                    let query = TermQuery::new(
                        Term::from_field_text(self.fields["article_id"], value.trim()),
                        IndexRecordOption::Basic,
                    );
                    for (key, hit) in
                        self.run_query(&query, "identifier", value, 20.0, searcher, allowed)?
                    {
                        matches.entry(key).or_insert(hit);
                    }
                    matches
                } else {
                    self.text_matches(
                        field,
                        value,
                        matches!(expr, SearchExpr::Phrase { .. }),
                        searcher,
                        allowed,
                    )?
                };
                if same_child {
                    return Ok(docs);
                }
                let mut out: Matches<'_> = FastMap::default();
                for hit in docs.into_values() {
                    let key = hit.candidate.meta.article_id.as_str();
                    if out.get(&key).is_none_or(|previous| {
                        hit.candidate.score > previous.candidate.score
                            || (hit.candidate.score == previous.candidate.score
                                && evidence_order(&hit.candidate, &previous.candidate).is_lt())
                    }) {
                        out.insert(key, hit);
                    }
                }
                Ok(out)
            }
        }
    }

    fn text_matches(
        &self,
        field: SearchField,
        value: &str,
        phrase: bool,
        searcher: &Searcher,
        allowed: Option<&HashSet<String>>,
    ) -> Result<Matches<'_>> {
        let phrase = phrase || crate::search_analyzer::adjacent_symbol(value);
        let fields: Vec<(&'static str, f32)> = match field {
            SearchField::Any => vec![
                ("title", 4.0),
                ("abstract", 2.0),
                ("creator", 0.8),
                ("publication", 0.8),
                ("identifier", 0.8),
                ("keyword", 1.0),
                ("tag", 1.0),
                ("year_text", 0.3),
                ("annotation", 1.4),
                ("fulltext", 1.0),
            ],
            SearchField::Title => vec![("title", 4.0)],
            SearchField::Abstract => vec![("abstract", 2.0)],
            SearchField::Creator => vec![("creator", 0.8)],
            SearchField::Publication => vec![("publication", 0.8)],
            SearchField::Keyword => vec![("keyword", 1.0)],
            SearchField::Tag => vec![("tag", 1.0)],
            SearchField::Note => vec![("annotation", 1.4)],
            SearchField::AttachmentContent => vec![("fulltext", 1.0)],
            _ => unreachable!(),
        };
        let terms = if phrase {
            vec![value.to_owned()]
        } else {
            unique_query_terms(value)
        };
        let sequences = if phrase {
            vec![terms]
        } else {
            terms.into_iter().map(|term| vec![term]).collect()
        };
        let mut result: Option<Matches> = None;
        for terms in sequences {
            let mut per_token: Matches<'_> = FastMap::default();
            for (field, boost) in &fields {
                let text = terms.join(" ");
                let indexed = if phrase || text.chars().any(crate::search_analyzer::is_han) {
                    crate::search_analyzer::position_tokens(&text)
                        .iter()
                        .map(|token| {
                            Term::from_field_text(
                                self.fields[field],
                                &crate::search_analyzer::position_term(token),
                            )
                        })
                        .collect::<Vec<_>>()
                } else {
                    terms
                        .iter()
                        .map(|term| Term::from_field_text(self.fields[field], term))
                        .collect::<Vec<_>>()
                };
                let mut query: Box<dyn Query> = if indexed.len() == 1 {
                    Box::new(TermQuery::new(
                        indexed[0].clone(),
                        IndexRecordOption::WithFreqs,
                    ))
                } else {
                    Box::new(PhraseQuery::new(indexed))
                };
                if !phrase && text.chars().any(crate::search_analyzer::is_han) {
                    let word = TermQuery::new(
                        Term::from_field_text(self.fields[field], &text),
                        IndexRecordOption::WithFreqs,
                    );
                    query = Box::new(BooleanQuery::new(vec![
                        (Occur::Must, query),
                        (
                            Occur::Should,
                            Box::new(BoostQuery::new(Box::new(word), 1.0)),
                        ),
                    ]));
                }
                for (key, hit) in self.run_query(
                    query.as_ref(),
                    field,
                    &terms.join(" "),
                    *boost,
                    searcher,
                    allowed,
                )? {
                    if let Some(previous) = per_token.get_mut(&key) {
                        merge(previous, hit);
                    } else {
                        per_token.insert(key, hit);
                    }
                }
            }
            if let Some(previous) = &mut result {
                previous.retain(|key, hit| {
                    if let Some(next) = per_token.remove(key) {
                        merge(hit, next);
                        true
                    } else {
                        false
                    }
                });
            } else {
                result = Some(per_token);
            }
        }
        Ok(result.unwrap_or_default())
    }

    fn run_query(
        &self,
        query: &dyn Query,
        field: &'static str,
        snippet_query: &str,
        boost: f32,
        searcher: &Searcher,
        allowed: Option<&HashSet<String>>,
    ) -> Result<Matches<'_>> {
        let mut out: Matches<'_> = FastMap::default();
        if searcher.num_docs() == 0 {
            return Ok(out);
        }
        let shared_snippet: Arc<str> = snippet_query.into();
        for (score, address) in searcher.search(query, &AllScoredDocs)? {
            let id = self
                .doc_ids
                .get(&searcher.segment_reader(address.segment_ord).segment_id())
                .and_then(|ids| ids.get(address.doc_id as usize))
                .ok_or_else(|| {
                    Error::Unknown("search document identity cache does not match reader".into())
                })?;
            if let Some(meta) = self.metadata.get(id.as_str()) {
                if allowed.is_some_and(|allowed| !allowed.contains(&meta.article_id)) {
                    continue;
                }
                let mut candidate = CandidateRef::new(meta);
                candidate.score = (score * boost) as f64;
                candidate.matched_field = field.into();
                candidate.snippet_query = shared_snippet.clone();
                out.insert(
                    id.as_str(),
                    Match {
                        best_score: candidate.score,
                        candidate,
                    },
                );
            }
        }
        Ok(out)
    }
}

// Intermediate term/field matches borrow immutable metadata. Only the
// selected output candidates allocate owned strings; scores/evidence remain
// local to the evaluation and never mutate the reader's metadata.
#[derive(Clone)]
struct CandidateRef<'a> {
    meta: &'a SearchCandidate,
    score: f64,
    matched_field: std::borrow::Cow<'a, str>,
    snippet_query: Arc<str>,
}
impl<'a> CandidateRef<'a> {
    fn new(meta: &'a SearchCandidate) -> Self {
        static EMPTY: std::sync::LazyLock<Arc<str>> = std::sync::LazyLock::new(|| Arc::from(""));
        Self {
            meta,
            score: meta.score,
            matched_field: std::borrow::Cow::Borrowed(&meta.matched_field),
            snippet_query: if meta.snippet_query.is_empty() {
                EMPTY.clone()
            } else {
                Arc::from(meta.snippet_query.as_str())
            },
        }
    }
    fn into_owned(self) -> SearchCandidate {
        let mut candidate = self.meta.clone();
        candidate.score = self.score;
        candidate.matched_field = self.matched_field.into_owned();
        candidate.snippet_query = self.snippet_query.to_string();
        candidate
    }
}
impl std::ops::Deref for CandidateRef<'_> {
    type Target = SearchCandidate;
    fn deref(&self) -> &Self::Target {
        self.meta
    }
}

type Matches<'a> = FastMap<&'a str, Match<'a>>;
#[derive(Clone)]
struct Match<'a> {
    candidate: CandidateRef<'a>,
    best_score: f64,
}
fn evidence_order(a: &SearchCandidate, b: &SearchCandidate) -> std::cmp::Ordering {
    fn priority(kind: &str) -> u8 {
        match kind {
            "item" => 0,
            "attachment" => 1,
            _ => 2,
        }
    }
    priority(&a.kind)
        .cmp(&priority(&b.kind))
        .then_with(|| a.doc_id.cmp(&b.doc_id))
}
fn merge<'a>(previous: &mut Match<'a>, hit: Match<'a>) {
    let score = previous.candidate.score + hit.candidate.score;
    if hit.best_score > previous.best_score
        || (hit.best_score == previous.best_score
            && evidence_order(&hit.candidate, &previous.candidate).is_lt())
    {
        *previous = hit;
    }
    previous.candidate.score = score;
}

pub(crate) fn normalize_identifier(value: &str) -> String {
    let value = value.trim().to_lowercase();
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
        "pmid:",
        "pmcid:",
        "arxiv:",
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            return rest.trim().to_owned();
        }
    }
    value
}
fn invalid(message: &str) -> Error {
    Error::InvalidSearchQuery(message.into())
}
pub(crate) fn parse_year(value: &str) -> Result<i64> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|year| (0..=9999).contains(year))
        .ok_or_else(|| invalid("year must be an integer between 0 and 9999"))
}
pub(crate) fn validate(expr: &SearchExpr, depth: usize, nodes: &mut usize) -> Result<()> {
    *nodes += 1;
    if depth > 32 || *nodes > 128 {
        return Err(invalid("query exceeds 128 clauses or 32 nesting levels"));
    }
    match expr {
        SearchExpr::And { clauses } | SearchExpr::Or { clauses } => {
            if clauses.is_empty() {
                return Err(invalid("Boolean groups must not be empty"));
            }
            for clause in clauses {
                validate(clause, depth + 1, nodes)?;
            }
        }
        SearchExpr::Not { clause } => validate(clause, depth + 1, nodes)?,
        SearchExpr::Range { field, gte, lte } => {
            if *field != SearchField::Year || (gte.is_none() && lte.is_none()) {
                return Err(invalid(
                    "ranges require the year field and at least one bound",
                ));
            }
            let lower = gte.as_deref().map(parse_year).transpose()?;
            let upper = lte.as_deref().map(parse_year).transpose()?;
            if lower.zip(upper).is_some_and(|(a, b)| a > b) {
                return Err(invalid("year range lower bound exceeds upper bound"));
            }
        }
        SearchExpr::Term { value, .. } | SearchExpr::Phrase { value, .. } => {
            let terms = tokenize(value);
            if value.len() > 16384 || terms.is_empty() || terms.len() > 128 {
                return Err(invalid(
                    "text clauses require 1 to 128 terms and at most 16384 bytes",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod identity_cache_tests {
    use super::*;

    fn document(id: &str, title: &str) -> SourceDocument {
        SourceDocument {
            meta: SearchCandidate {
                doc_id: format!("{id}:item"),
                article_id: id.into(),
                anchor_key: id.into(),
                kind: "item".into(),
                title: title.into(),
                year: None,
                score: 0.,
                matched_field: "title".into(),
                snippet_query: String::new(),
            },
            texts: std::array::from_fn(|i| {
                if TEXT_FIELDS[i] == "title" {
                    title.into()
                } else {
                    String::new()
                }
            }),
            identifiers: Vec::new(),
        }
    }
    fn hits(index: &LocalIndex, value: &str) -> Vec<String> {
        index
            .search(
                Some(&SearchExpr::Term {
                    field: Some(SearchField::Title),
                    value: value.into(),
                }),
                false,
            )
            .unwrap()
            .into_iter()
            .map(|h| h.article_id)
            .collect()
    }
    fn verify_stored_identities(index: &LocalIndex) {
        let reader = index.reader.searcher();
        assert_eq!(index.doc_ids.len(), reader.segment_readers().len());
        for (ordinal, segment) in reader.segment_readers().iter().enumerate() {
            let ids = &index.doc_ids[&segment.segment_id()];
            for doc in 0..segment.max_doc() {
                if segment.is_deleted(doc) {
                    continue;
                }
                let stored: TantivyDocument = reader
                    .doc(DocAddress {
                        segment_ord: ordinal as u32,
                        doc_id: doc,
                    })
                    .unwrap();
                assert_eq!(
                    ids[doc as usize],
                    stored
                        .get_first(index.fields["doc_id"])
                        .unwrap()
                        .as_str()
                        .unwrap()
                );
            }
        }
    }
    #[test]
    fn unordered_collection_preserves_all_scores_and_filters_deleted_documents() {
        let mut index = LocalIndex::open(":memory:").unwrap();
        index
            .writer
            .set_merge_policy(Box::new(tantivy::indexer::NoMergePolicy));
        for batch in 0..2 {
            index
                .begin_update(Vec::new(), batch + 1, format!("token-{batch}"), false)
                .unwrap();
            index
                .append_documents(
                    (0..50)
                        .map(|i| {
                            document(
                                &format!("id-{batch}-{i}"),
                                &format!("gene {} immune", "gene ".repeat(i % 4)),
                            )
                        })
                        .collect(),
                )
                .unwrap();
            index.finish_update().unwrap();
        }
        index
            .begin_update(vec!["id-0-0".into()], 3, "deleted".into(), false)
            .unwrap();
        index.finish_update().unwrap();
        let reader = index.reader.searcher();
        let query = TermQuery::new(
            Term::from_field_text(index.fields["title"], "gene"),
            IndexRecordOption::WithFreqs,
        );
        let mut expected = reader
            .search(
                &query,
                &TopDocs::with_limit(reader.num_docs() as usize).order_by_score(),
            )
            .unwrap();
        let mut actual = reader.search(&query, &AllScoredDocs).unwrap();
        let order = |hit: &(f32, DocAddress)| (hit.1.segment_ord, hit.1.doc_id);
        expected.sort_by_key(order);
        actual.sort_by_key(order);
        assert_eq!(actual.len(), 99);
        assert_eq!(actual, expected);
    }

    #[test]
    fn early_allowed_filter_preserves_article_scores_and_cross_object_matching() {
        let mut index = LocalIndex::open(":memory:").unwrap();
        let mut a_child = document("a", "immune evidence");
        a_child.meta.doc_id = "a:attachment".into();
        a_child.meta.kind = "attachment".into();
        index
            .begin_update(Vec::new(), 1, "token".into(), true)
            .unwrap();
        index
            .append_documents(vec![
                document("a", "gene editing"),
                a_child,
                document("b", "immune gene expression"),
                document("c", "gene expression"),
            ])
            .unwrap();
        index.finish_update().unwrap();
        let allowed = HashSet::from(["a".to_owned(), "c".to_owned()]);
        for expression in [
            SearchExpr::Or {
                clauses: vec![
                    SearchExpr::Term {
                        field: None,
                        value: "gene".into(),
                    },
                    SearchExpr::Term {
                        field: None,
                        value: "immune".into(),
                    },
                ],
            },
            SearchExpr::And {
                clauses: vec![
                    SearchExpr::Term {
                        field: None,
                        value: "gene".into(),
                    },
                    SearchExpr::Term {
                        field: None,
                        value: "immune".into(),
                    },
                ],
            },
        ] {
            let expected = index
                .search(Some(&expression), false)
                .unwrap()
                .into_iter()
                .filter(|h| allowed.contains(&h.article_id))
                .map(|h| {
                    (
                        h.article_id,
                        h.doc_id,
                        h.score,
                        h.matched_field,
                        h.snippet_query,
                    )
                })
                .collect::<Vec<_>>();
            let actual = index
                .search_in(Some(&expression), false, Some(&allowed))
                .unwrap()
                .into_iter()
                .map(|h| {
                    (
                        h.article_id,
                        h.doc_id,
                        h.score,
                        h.matched_field,
                        h.snippet_query,
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, expected);
            let limited = index
                .search_top_in(&expression, &allowed, 1)
                .unwrap()
                .into_iter()
                .map(|h| {
                    (
                        h.article_id,
                        h.doc_id,
                        h.score,
                        h.matched_field,
                        h.snippet_query,
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(limited, expected.into_iter().take(1).collect::<Vec<_>>());
        }
        assert!(
            index
                .search_in(None, false, Some(&HashSet::new()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn identity_cache_survives_updates_deletion_rollback_and_segment_merge() {
        let mut index = LocalIndex::open(":memory:").unwrap();
        index
            .writer
            .set_merge_policy(Box::new(tantivy::indexer::NoMergePolicy));
        for (revision, id, title) in [(1, "a", "alphamarker"), (2, "b", "betamarker")] {
            index
                .begin_update(
                    vec![id.into()],
                    revision,
                    format!("token-{revision}"),
                    false,
                )
                .unwrap();
            index.append_documents(vec![document(id, title)]).unwrap();
            index.finish_update().unwrap();
        }
        verify_stored_identities(&index);
        let b_ids = index
            .doc_ids
            .values()
            .find(|ids| ids.iter().any(|id| id == "b:item"))
            .unwrap()
            .clone();
        index
            .begin_update(vec!["a".into()], 3, "token-3".into(), false)
            .unwrap();
        index
            .append_documents(vec![document("a", "replacementmarker")])
            .unwrap();
        index.finish_update().unwrap();
        assert!(index.doc_ids.values().any(|ids| Arc::ptr_eq(ids, &b_ids)));
        assert!(hits(&index, "alphamarker").is_empty());
        assert_eq!(hits(&index, "replacementmarker"), ["a"]);
        verify_stored_identities(&index);
        let segments = index
            .reader
            .searcher()
            .segment_readers()
            .iter()
            .map(|s| s.segment_id())
            .collect::<Vec<_>>();
        assert!(segments.len() > 1);
        index.writer.merge(&segments).wait().unwrap();
        index.reader.reload().unwrap();
        index.refresh_doc_ids().unwrap();
        assert_eq!(index.doc_ids.len(), 1);
        verify_stored_identities(&index);
        assert_eq!(hits(&index, "betamarker"), ["b"]);
        index
            .begin_update(vec!["b".into()], 4, "token-4".into(), false)
            .unwrap();
        index.fail_next_commit = true;
        assert!(index.finish_update().is_err());
        index.abort_update().unwrap();
        assert_eq!(hits(&index, "betamarker"), ["b"]);
        verify_stored_identities(&index);
        index
            .begin_update(vec!["b".into()], 4, "token-4".into(), false)
            .unwrap();
        index.finish_update().unwrap();
        assert!(hits(&index, "betamarker").is_empty());
        assert_eq!(hits(&index, "replacementmarker"), ["a"]);
        verify_stored_identities(&index);
    }
}
