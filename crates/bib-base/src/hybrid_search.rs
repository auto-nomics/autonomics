//! Fixed, independently evaluated weighted RRF. Hard query, scope and filters
//! define the same allowed article universe before either branch's Top-K.
use crate::{
    Error, Result,
    bib_base::{BibCore, SearchCandidate},
    semantic_index::SemanticChunk,
    semantic_service::{PreparedSemanticQuery, SemanticConfig},
};
use bib_types::{
    SearchExpr, SearchFacets, SearchHit, SearchMode, SearchRecall, SearchRequestV2,
    SearchRetrieval, SemanticSearchEvidence,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

const DEPTH: usize = 100;
const RRF_K: usize = 10;
const DENSE_WEIGHT: f64 = 4.;

pub(crate) struct Evaluation {
    pub hits: Vec<SearchCandidate>,
    pub facets: Option<SearchFacets>,
    pub retrieval: Option<SearchRetrieval>,
    pub semantic_chunks: HashMap<String, SemanticChunk>,
}
fn invalid(message: &str) -> Error {
    Error::InvalidSearchQuery(message.into())
}
fn stale() -> Error {
    Error::SearchConflict(
        "semantic index or configuration is stale; refresh after indexing completes".into(),
    )
}

/// Soft lexical recall for a topic, shared by production hybrid retrieval and
/// its lexical baseline. Hard query parsing is separate.
pub fn topic_expression(text: &str) -> Result<SearchExpr> {
    if text.len() > 16384 {
        return Err(invalid("semantic_query exceeds 16384 bytes"));
    }
    let mut terms = Vec::new();
    for word in text
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
    {
        if !word.chars().any(crate::search_analyzer::is_han) {
            // Preserve the English-selected expression exactly, including
            // scientific symbols and repeated words.
            terms.push(word.to_owned());
            continue;
        }
        // Han sentences have no whitespace boundaries. Expose words using the
        // same locked dictionary as indexing before applying OR; otherwise a
        // single Term implicitly ANDs the whole sentence.
        let mut start = 0;
        let mut han = word
            .chars()
            .next()
            .is_some_and(crate::search_analyzer::is_han);
        for (at, ch) in word.char_indices().skip(1) {
            let next_han = crate::search_analyzer::is_han(ch);
            if next_han != han {
                append_topic_run(&word[start..at], han, &mut terms);
                start = at;
                han = next_han;
            }
        }
        append_topic_run(&word[start..], han, &mut terms);
    }
    let clauses = terms
        .into_iter()
        .map(|value| SearchExpr::Term { field: None, value })
        .collect();
    let expr = SearchExpr::Or { clauses };
    crate::local_index::validate(&expr, 0, &mut 0)?;
    Ok(expr)
}

fn append_topic_run(run: &str, han: bool, terms: &mut Vec<String>) {
    if han {
        terms.extend(
            crate::search_analyzer::analyze(run)
                .into_iter()
                .map(|token| token.text),
        );
    } else if run.chars().any(char::is_alphanumeric) {
        terms.push(run.to_owned());
    }
}

pub(crate) fn generation(
    revision: u64,
    token: &str,
    prepared: Option<&PreparedSemanticQuery>,
) -> String {
    let lexical = format!("tantivy-v2:{revision}:{token}");
    match prepared {
        None => lexical,
        Some(prepared) => format!(
            "{lexical}:hybrid-rrf-v1-d100-k10-w4:{}:{}:{}",
            prepared.snapshot.signature,
            prepared.snapshot.generation,
            prepared.configuration_generation
        ),
    }
}

impl BibCore {
    pub(crate) async fn validate_semantic_configuration(
        &self,
        prepared: &PreparedSemanticQuery,
    ) -> Result<()> {
        let service = self.semantic.as_ref().ok_or_else(stale)?;
        let current = service.status();
        if !current.config.enabled
            || current.configuration_generation != prepared.configuration_generation
            || current.config != prepared.configuration
        {
            return Err(stale());
        }
        if crate::semantic_service::active_store(&self.conn(), &prepared.snapshot.signature).await?
            != prepared.store_generation
        {
            return Err(stale());
        }
        // Also inspect the canonical configuration: another process may have
        // committed a switch before this process's observation interval.
        let mut rows = self
            .conn()
            .query(
                "SELECT value,generation FROM bibliographySemanticConfig WHERE id=1",
                (),
            )
            .await?;
        let row = rows.next().await?.ok_or_else(stale)?;
        let stored: SemanticConfig =
            serde_json::from_str(&row.get::<String>(0)?).map_err(|_| stale())?;
        if row.get::<String>(1)? != prepared.configuration_generation
            || stored != prepared.configuration
        {
            return Err(stale());
        }
        Ok(())
    }

    pub(crate) async fn evaluate_search(
        &self,
        request: &SearchRequestV2,
        prepared: Option<&PreparedSemanticQuery>,
    ) -> Result<Evaluation> {
        if request.mode == SearchMode::Lexical {
            if prepared.is_some() {
                return Err(invalid(
                    "unexpected semantic preparation for lexical search",
                ));
            }
            let (hits, facets) = self.scoped_search_candidates(request).await?;
            return Ok(Evaluation {
                hits,
                facets,
                retrieval: None,
                semantic_chunks: HashMap::new(),
            });
        }
        let prepared = prepared
            .ok_or_else(|| invalid("hybrid query requires preparation outside the search actor"))?;
        if request.semantic_query.as_deref() != Some(prepared.query.as_str()) {
            return Err(invalid("semantic preparation belongs to another topic"));
        }
        self.validate_semantic_configuration(prepared).await?;
        let (revision, token) = crate::semantic_index::source_version(&self.conn()).await?;
        if prepared.snapshot.revision != revision || prepared.snapshot.token != token {
            return Err(stale());
        }
        let (hard, facets) = self.scoped_search_candidates(request).await?;
        let eligible_articles = hard.len();
        let allowed = std::sync::Arc::new(
            hard.iter()
                .map(|hit| hit.article_id.clone())
                .collect::<HashSet<_>>(),
        );
        let topic = topic_expression(&prepared.query)?;
        let mut lexical = self.ranked_local_in(topic, allowed.clone(), DEPTH).await?;
        lexical.retain(|hit| allowed.contains(&hit.article_id));
        lexical.truncate(DEPTH);
        let snapshot = prepared.snapshot.clone();
        let vector = prepared.vector.clone();
        let dense = tokio::task::spawn_blocking(move || snapshot.rank(&vector, &allowed, DEPTH))
            .await
            .map_err(|error| Error::Unknown(format!("semantic ranking task: {error}")))??;
        let mut hits = HashMap::<String, SearchCandidate>::new();
        let mut details = BTreeMap::<String, SearchRecall>::new();
        for (position, mut hit) in lexical.into_iter().enumerate() {
            let rank = position + 1;
            details
                .entry(hit.article_id.clone())
                .or_default()
                .lexical_rank = Some(rank);
            hit.score = 1. / (RRF_K + rank) as f64;
            hits.insert(hit.article_id.clone(), hit);
        }
        let hard = hard
            .into_iter()
            .map(|hit| (hit.article_id.clone(), hit))
            .collect::<HashMap<_, _>>();
        let mut semantic_chunks = HashMap::new();
        for (position, (chunk, _cosine)) in dense.into_iter().enumerate() {
            let rank = position + 1;
            details
                .entry(chunk.article_id.clone())
                .or_default()
                .semantic_rank = Some(rank);
            let hit = hits.entry(chunk.article_id.clone()).or_insert_with(|| {
                let mut hit = hard[&chunk.article_id].clone();
                hit.doc_id = chunk.doc_id.clone();
                hit.anchor_key = chunk.anchor_key.clone();
                hit.kind = chunk.kind.clone();
                hit.matched_field = match chunk.kind.as_str() {
                    "item" => "abstract",
                    "annotation" => "annotation",
                    _ => "fulltext",
                }
                .into();
                hit.snippet_query.clear();
                hit.score = 0.;
                hit
            });
            hit.score += DENSE_WEIGHT / (RRF_K + rank) as f64;
            semantic_chunks.insert(chunk.article_id.clone(), chunk);
        }
        let mut hits = hits.into_values().collect::<Vec<_>>();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.article_id.cmp(&b.article_id))
        });
        Ok(Evaluation {
            hits,
            facets,
            semantic_chunks,
            retrieval: Some(SearchRetrieval {
                algorithm: "weighted_rrf_v1".into(),
                total_kind: "candidate_pool".into(),
                facets_basis: "hard_query_disjunctive_filters".into(),
                eligible_articles,
                candidate_depth: DEPTH,
                lexical_weight: 1.,
                semantic_weight: DENSE_WEIGHT,
                rrf_constant: RRF_K,
                semantic_generation: prepared.snapshot.generation.clone(),
                configuration_generation: prepared.configuration_generation.clone(),
                indexed_chunks: prepared.snapshot.chunks.len(),
                hits: details,
            }),
        })
    }

    pub(crate) async fn render_semantic_page(
        &self,
        candidates: &[SearchCandidate],
        rendered: &mut [SearchHit],
        retrieval: &mut Option<SearchRetrieval>,
        chunks: &HashMap<String, SemanticChunk>,
    ) -> Result<()> {
        let Some(retrieval) = retrieval else {
            return Ok(());
        };
        let ids = candidates
            .iter()
            .map(|hit| hit.article_id.as_str())
            .collect::<HashSet<_>>();
        retrieval.hits.retain(|id, _| ids.contains(id.as_str()));
        for (candidate, hit) in candidates.iter().zip(rendered) {
            let Some(chunk) = chunks.get(&candidate.article_id) else {
                continue;
            };
            let evidence = self.render_semantic_evidence(chunk).await?;
            let detail = retrieval
                .hits
                .get_mut(&candidate.article_id)
                .expect("candidate recall detail");
            if detail.lexical_rank.is_none() {
                hit.snippet = evidence.snippet.clone();
                hit.highlight_ranges.clear();
                hit.matched_field = Some(evidence.matched_field.clone());
                hit.anchor_key = Some(evidence.anchor_key.clone());
            }
            detail.semantic_evidence = Some(evidence);
        }
        Ok(())
    }

    pub(crate) async fn render_semantic_evidence(
        &self,
        chunk: &SemanticChunk,
    ) -> Result<SemanticSearchEvidence> {
        let projection = crate::search_projection::select(Some("a.id=?1"));
        let mut rows = self
            .conn()
            .query(
                format!("SELECT title,abstract,body FROM ({projection}) WHERE docID=?2"),
                turso::params![chunk.article_id.clone(), chunk.doc_id.clone()],
            )
            .await?;
        let row = rows.next().await?.ok_or_else(stale)?;
        let title: String = row.get(0)?;
        let body: String = row.get(if chunk.kind == "item" { 1 } else { 2 })?;
        let hash = format!(
            "{:x}",
            Sha256::digest(format!("{title}\n{body}").as_bytes())
        );
        if hash != chunk.content_hash {
            return Err(stale());
        }
        let window = body
            .get(chunk.start_byte..chunk.end_byte)
            .ok_or_else(stale)?;
        let (field, text) = if chunk.kind == "item" && body.is_empty() {
            ("title", title.as_str())
        } else {
            (
                match chunk.kind.as_str() {
                    "item" => "abstract",
                    "annotation" => "annotation",
                    _ => "fulltext",
                },
                window,
            )
        };
        Ok(SemanticSearchEvidence {
            doc_id: chunk.doc_id.clone(),
            anchor_key: chunk.anchor_key.clone(),
            kind: chunk.kind.clone(),
            matched_field: field.into(),
            content_hash: hash,
            start_byte: chunk.start_byte,
            end_byte: chunk.end_byte,
            snippet: text.chars().take(240).collect(),
        })
    }
}

#[cfg(test)]
mod topic_tests {
    use super::*;
    #[test]
    fn english_topics_preserve_symbols_repetitions_and_the_frozen_expression() {
        for text in [
            "IL-6 IL-6 β-catenin immune regulation",
            "Given a scientific claim, retrieve papers.",
            "BRCA1 AND NOT cancer",
            "chr6:100-200 T-cell RNA-seq",
        ] {
            let previous = SearchExpr::Or {
                clauses: text
                    .split_whitespace()
                    .filter(|word| word.chars().any(char::is_alphanumeric))
                    .map(|value| SearchExpr::Term {
                        field: None,
                        value: value.into(),
                    })
                    .collect(),
            };
            assert_eq!(topic_expression(text).unwrap(), previous);
        }
    }
    #[tokio::test]
    async fn han_words_are_soft_alternatives_and_explicit_boolean_conditions_still_apply() {
        let db = crate::BibBase::open_in_memory().await.unwrap();
        for (id, title) in [
            ("tumor", "肿瘤研究"),
            ("immune", "免疫研究"),
            ("other", "天文研究"),
        ] {
            db.upsert_article(&bib_types::Article::new(id, title))
                .await
                .unwrap();
        }
        let topic = topic_expression("肿瘤免疫").unwrap();
        let page = db
            .search_v2(SearchRequestV2 {
                expression: Some(topic.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.total, 2);
        let hard = SearchExpr::Not {
            clause: Box::new(SearchExpr::Term {
                field: Some(bib_types::SearchField::Title),
                value: "肿瘤".into(),
            }),
        };
        let page = db
            .search_v2(SearchRequestV2 {
                expression: Some(SearchExpr::And {
                    clauses: vec![hard, topic],
                }),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.hits[0].article_id, "immune");
    }
    #[test]
    fn mixed_han_and_scientific_symbols_keep_the_symbol_adjacent() {
        let SearchExpr::Or { clauses } = topic_expression("研究IL-6调控").unwrap() else {
            panic!("expected OR")
        };
        assert!(clauses.contains(&SearchExpr::Term {
            field: None,
            value: "IL-6".into()
        }));
        assert!(!clauses.contains(&SearchExpr::Term {
            field: None,
            value: "IL".into()
        }));
        assert!(topic_expression(&"免疫".repeat(8193)).is_err());
    }
}
