//! Conservative strong-ID union with immutable source provenance. Similar
//! titles and conflicting publication identities only create review links.
use bib_types::{Article, ArticleSource, IdKind, discovery::*};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

struct Entry {
    id: String,
    records: Vec<DiscoveryRecord>,
    related: BTreeMap<usize, String>,
    possible: BTreeSet<usize>,
}
#[derive(Default)]
pub(crate) struct CandidateStore {
    entries: Vec<Option<Entry>>,
    parents: Vec<usize>,
    ids: HashMap<String, usize>,
    strong: HashMap<String, BTreeSet<usize>>,
    source_ids: HashMap<(String, String), usize>,
    titles: HashMap<String, Vec<usize>>,
    count: usize,
}
#[derive(Default)]
pub(crate) struct MergeDelta {
    pub upserted_ids: Vec<String>,
    pub removed_ids: Vec<String>,
}

fn doi(article: &Article) -> BTreeSet<String> {
    article
        .identifiers
        .iter()
        .filter(|id| matches!(id.kind, IdKind::Doi | IdKind::Biorxiv))
        .filter_map(identifier_key)
        .map(|key| key.strip_prefix("doi:").expect("DOI key prefix").to_owned())
        .collect()
}

pub(crate) fn identity_keys(article: &Article) -> Vec<String> {
    article
        .identifiers
        .iter()
        .filter_map(identifier_key)
        .collect()
}
pub(crate) fn identifier_key(id: &bib_types::Identifier) -> Option<String> {
    let value = id.value.trim().to_lowercase();
    if value.is_empty() {
        return None;
    }
    let payload = |prefixes: &[&str]| -> String {
        for prefix in prefixes {
            if let Some(rest) = value.strip_prefix(prefix) {
                return if prefix.starts_with("http") {
                    rest.split(['?', '#'])
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .trim_end_matches('/')
                        .to_owned()
                } else {
                    rest.trim().to_owned()
                };
            }
        }
        value.clone()
    };
    Some(match id.kind {
        IdKind::Doi | IdKind::Biorxiv => {
            let mut doi = value.as_str();
            for prefix in [
                "https://doi.org/",
                "http://doi.org/",
                "https://dx.doi.org/",
                "http://dx.doi.org/",
                "doi:",
            ] {
                if let Some(rest) = value.strip_prefix(prefix) {
                    doi = rest.trim();
                    break;
                }
            }
            if !doi.split_once('/').is_some_and(|(prefix, suffix)| {
                prefix.strip_prefix("10.").is_some_and(|registrant| {
                    (4..=9).contains(&registrant.len())
                        && registrant.bytes().all(|b| b.is_ascii_digit())
                }) && !suffix.is_empty()
                    && !doi.chars().any(char::is_whitespace)
            }) {
                return None;
            }
            format!("doi:{doi}")
        }
        IdKind::Pmid => {
            let value = payload(&[
                "pmid:",
                "https://pubmed.ncbi.nlm.nih.gov/",
                "http://pubmed.ncbi.nlm.nih.gov/",
            ]);
            format!(
                "pmid:{}",
                value.trim().parse::<u64>().ok().filter(|v| *v > 0)?
            )
        }
        IdKind::Pmc => {
            let value = payload(&[
                "https://pmc.ncbi.nlm.nih.gov/articles/",
                "https://www.ncbi.nlm.nih.gov/pmc/articles/",
                "pmcid:",
            ]);
            format!(
                "pmc:{}",
                value
                    .trim_start_matches("pmc")
                    .parse::<u64>()
                    .ok()
                    .filter(|v| *v > 0)?
            )
        }
        IdKind::Arxiv => {
            let value = crate::discovery_arxiv::identifier(&id.value)?;
            format!("arxiv:{}", crate::discovery_arxiv::version_base(&value))
        }
        IdKind::OpenAlex => {
            let value = payload(&["https://openalex.org/", "http://openalex.org/", "openalex:"]);
            if value.is_empty() {
                return None;
            }
            format!("openalex:{}", value.to_uppercase())
        }
        IdKind::S2 => format!("s2:{value}"),
        IdKind::Embase => format!("embase:{}", id.value.trim()),
        IdKind::Other => return None,
    })
}

pub(crate) fn preprint(article: &Article) -> bool {
    matches!(
        article.source,
        ArticleSource::Arxiv | ArticleSource::Biorxiv
    ) || article
        .pub_types
        .iter()
        .any(|v| v.to_lowercase().contains("preprint"))
        || article
            .journal
            .as_ref()
            .is_some_and(|v| v.to_lowercase().contains("preprint"))
}
pub(crate) fn published(article: &Article) -> bool {
    !preprint(article)
        && (article.source == ArticleSource::Pubmed
            || article.pub_types.iter().any(|v| {
                matches!(
                    v.to_lowercase().as_str(),
                    "article"
                        | "journal article"
                        | "journal-article"
                        | "journal_article"
                        | "journalarticle"
                        | "review"
                )
            }))
}
pub(crate) fn compatible(a: &Article, b: &Article) -> bool {
    let ad = doi(a);
    let bd = doi(b);
    if !ad.is_empty() && !bd.is_empty() && ad != bd {
        return false;
    }
    if ad.is_disjoint(&bd) && ((preprint(a) && published(b)) || (published(a) && preprint(b))) {
        return false;
    }
    true
}
fn normalized_title(article: &Article) -> String {
    article
        .title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

impl CandidateStore {
    fn score(&self, index: usize) -> f64 {
        let mut ranks: BTreeMap<&str, usize> = BTreeMap::new();
        for r in &self.entry(index).records {
            let best = ranks.entry(&r.source).or_insert(r.source_rank);
            *best = (*best).min(r.source_rank);
        }
        ranks
            .values()
            .map(|rank| 1.0 / (60.0 + (*rank).max(1) as f64))
            .sum()
    }
    pub fn len(&self) -> usize {
        self.count
    }
    fn root(&self, mut index: usize) -> usize {
        while self.parents[index] != index {
            index = self.parents[index];
        }
        index
    }
    fn entry(&self, index: usize) -> &Entry {
        self.entries[self.root(index)]
            .as_ref()
            .expect("candidate root exists")
    }
    fn new_entry(&mut self) -> usize {
        let index = self.entries.len();
        let id = uuid::Uuid::new_v4().to_string();
        self.entries.push(Some(Entry {
            id: id.clone(),
            records: Vec::new(),
            related: BTreeMap::new(),
            possible: BTreeSet::new(),
        }));
        self.parents.push(index);
        self.ids.insert(id, index);
        self.count += 1;
        index
    }
    pub fn ingest(&mut self, records: Vec<DiscoveryRecord>) -> MergeDelta {
        let mut delta = MergeDelta::default();
        let mut changed = BTreeSet::new();
        for mut record in records {
            // SDK constructors stamp local acquisition times; those are not
            // bibliographic changes. Provenance already has retrieved_at.
            record.article.created_at = None;
            record.article.updated_at = None;
            let id_keys = identity_keys(&record.article);
            let mut matches = id_keys
                .iter()
                .filter_map(|key| self.strong.get(key))
                .flat_map(|indices| indices.iter().map(|i| self.root(*i)))
                .collect::<BTreeSet<_>>();
            if let Some(index) = self
                .source_ids
                .get(&(record.source.clone(), record.article.id.clone()))
            {
                matches.insert(self.root(*index));
            }
            let compatible_roots = matches
                .iter()
                .copied()
                .filter(|i| {
                    self.entry(*i)
                        .records
                        .iter()
                        .all(|existing| compatible(&existing.article, &record.article))
                })
                .collect::<Vec<_>>();
            // A bridge with contradictory existing identities is ambiguous;
            // never let arrival order choose which publication it belongs to.
            let bridge_safe = compatible_roots.iter().enumerate().all(|(n, a)| {
                compatible_roots.iter().skip(n + 1).all(|b| {
                    self.entry(*a).records.iter().all(|ar| {
                        self.entry(*b)
                            .records
                            .iter()
                            .all(|br| compatible(&ar.article, &br.article))
                    })
                })
            });
            let root = if bridge_safe {
                compatible_roots
                    .first()
                    .copied()
                    .unwrap_or_else(|| self.new_entry())
            } else {
                self.new_entry()
            };
            if bridge_safe {
                for other in compatible_roots.into_iter().filter(|i| *i != root) {
                    let removed = self.entries[other].take().expect("candidate exists");
                    delta.removed_ids.push(removed.id);
                    self.parents[other] = root;
                    self.count -= 1;
                    let target = self.entries[root].as_mut().unwrap();
                    target.records.extend(removed.records);
                    target.related.extend(removed.related);
                    target.possible.extend(removed.possible);
                }
            }
            for other in matches {
                let other = self.root(other);
                if other == root {
                    continue;
                }
                self.entries[root].as_mut().unwrap().related.insert(other,"Shared identifier with a different publication identity; review versions before merging.".into());
                self.entries[other].as_mut().unwrap().related.insert(root,"Shared identifier with a different publication identity; review versions before merging.".into());
                changed.insert(other);
            }
            let title = normalized_title(&record.article);
            if !title.is_empty() {
                let similar = self.titles.get(&title).cloned().unwrap_or_default();
                for other in similar.into_iter().take(16) {
                    let other = self.root(other);
                    if other == root {
                        continue;
                    }
                    self.entries[root].as_mut().unwrap().possible.insert(other);
                    self.entries[other].as_mut().unwrap().possible.insert(root);
                    changed.insert(other);
                }
                let bucket = self.titles.entry(title).or_default();
                if bucket.len() < 32 && !bucket.contains(&root) {
                    bucket.push(root);
                }
            }
            self.source_ids
                .insert((record.source.clone(), record.article.id.clone()), root);
            let entry = self.entries[root].as_mut().unwrap();
            if !entry
                .records
                .iter()
                .any(|v| v.source == record.source && v.article == record.article)
            {
                entry.records.push(record);
            }
            for key in id_keys {
                self.strong.entry(key).or_default().insert(root);
            }
            changed.insert(root);
        }
        let changed = changed
            .into_iter()
            .map(|i| self.root(i))
            .collect::<BTreeSet<_>>();
        delta.upserted_ids = changed
            .into_iter()
            .map(|i| self.entry(i).id.clone())
            .collect();
        delta
    }
    pub fn candidate(&self, id: &str) -> Option<DiscoveryCandidate> {
        self.ids
            .get(id)
            .map(|index| self.materialize(self.root(*index)))
    }
    fn materialize(&self, index: usize) -> DiscoveryCandidate {
        let entry = self.entry(index);
        let records = &entry.records;
        let mut by_source: BTreeMap<&str, usize> = BTreeMap::new();
        for record in records {
            let rank = by_source
                .entry(&record.source)
                .or_insert(record.source_rank);
            *rank = (*rank).min(record.source_rank);
        }
        let mut fields: BTreeMap<&str, Vec<FieldVariant>> = BTreeMap::new();
        for record in records {
            let a = &record.article;
            for (field, value) in [
                ("title", serde_json::json!(a.title)),
                ("year", serde_json::json!(a.year)),
                ("journal", serde_json::json!(a.journal)),
                ("abstract", serde_json::json!(a.abstract_text)),
                ("authors", serde_json::json!(a.authors)),
                ("publication_types", serde_json::json!(a.pub_types)),
            ] {
                if value.is_null()
                    || value == serde_json::json!("")
                    || value == serde_json::json!([])
                {
                    continue;
                }
                fields.entry(field).or_default().push(FieldVariant {
                    source: record.source.clone(),
                    source_record_id: a.id.clone(),
                    value,
                });
            }
        }
        let conflicts = fields
            .into_iter()
            .filter_map(|(field, variants)| {
                if variants
                    .iter()
                    .map(|v| v.value.to_string())
                    .collect::<HashSet<_>>()
                    .len()
                    < 2
                {
                    None
                } else {
                    Some(FieldConflict {
                        field: field.into(),
                        variants,
                    })
                }
            })
            .collect();
        let mut seen = BTreeSet::new();
        let related_versions = entry
            .related
            .iter()
            .filter_map(|(other, reason)| {
                let other = self.root(*other);
                if other == index || !seen.insert(other) {
                    None
                } else {
                    Some(CandidateLink {
                        candidate_id: self.entry(other).id.clone(),
                        reason: reason.clone(),
                    })
                }
            })
            .collect();
        let possible_duplicates = entry
            .possible
            .iter()
            .map(|i| self.root(*i))
            .filter(|i| *i != index)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|i| self.entry(i).id.clone())
            .collect();
        DiscoveryCandidate {
            id: entry.id.clone(),
            fingerprint: format!(
                "{:x}",
                <sha2::Sha256 as sha2::Digest>::digest(
                    serde_json::to_vec(records).expect("discovery records serialize")
                )
            ),
            article: records[0].article.clone(),
            records: records.clone(),
            conflicts,
            related_versions,
            possible_duplicates,
            rrf_score: by_source
                .values()
                .map(|rank| 1.0 / (60.0 + (*rank).max(1) as f64))
                .sum(),
            first_seen: index,
        }
    }
    pub fn results(
        &self,
        source: Option<&str>,
        order: DiscoveryResultOrder,
        offset: usize,
        limit: usize,
    ) -> (Vec<DiscoveryCandidate>, usize) {
        let mut indices = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.as_ref().is_some_and(|entry| {
                    source.is_none_or(|source| {
                        entry.records.iter().any(|record| record.source == source)
                    })
                })
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        match order {
            DiscoveryResultOrder::FirstSeen => {}
            DiscoveryResultOrder::Relevance => {
                let scores = indices
                    .iter()
                    .map(|i| (*i, self.score(*i)))
                    .collect::<HashMap<_, _>>();
                indices.sort_by(|a, b| scores[b].total_cmp(&scores[a]).then_with(|| a.cmp(b)));
            }
            DiscoveryResultOrder::Newest => indices.sort_by_key(|i| {
                (
                    std::cmp::Reverse(self.entry(*i).records[0].article.year.unwrap_or(0)),
                    *i,
                )
            }),
            DiscoveryResultOrder::SourceRank => indices.sort_by_key(|i| {
                (
                    self.entry(*i)
                        .records
                        .iter()
                        .filter(|r| source.is_none_or(|s| r.source == s))
                        .map(|r| r.source_rank)
                        .min()
                        .unwrap_or(usize::MAX),
                    *i,
                )
            }),
        }
        let total = indices.len();
        (
            indices
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|i| {
                    let mut candidate = self.materialize(i);
                    if let Some(source) = source {
                        if let Some(record) = candidate
                            .records
                            .iter()
                            .filter(|record| record.source == source)
                            .min_by_key(|record| record.source_rank)
                        {
                            candidate.article = record.article.clone();
                        }
                    }
                    candidate
                })
                .collect(),
            total,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifier_keys_normalize_known_wrappers_without_losing_literal_doi_suffixes() {
        for value in [
            "10.1000/Mixed",
            "HTTPS://DOI.ORG/10.1000/Mixed",
            "http://dx.doi.org/10.1000/Mixed",
        ] {
            assert_eq!(
                identifier_key(&bib_types::Identifier::doi(value)).as_deref(),
                Some("doi:10.1000/mixed")
            );
        }
        assert_ne!(
            identifier_key(&bib_types::Identifier::doi("10.1000/a?b")),
            identifier_key(&bib_types::Identifier::doi("10.1000/a?c"))
        );
        assert_ne!(
            identifier_key(&bib_types::Identifier::doi("10.1000/a/")),
            identifier_key(&bib_types::Identifier::doi("10.1000/a"))
        );
        for value in [
            "00101",
            "PMID:101",
            "https://pubmed.ncbi.nlm.nih.gov/101/?from=search",
        ] {
            assert_eq!(
                identifier_key(&bib_types::Identifier::pmid(value)).as_deref(),
                Some("pmid:101")
            );
        }
        assert!(identifier_key(&bib_types::Identifier::pmid("PMID:101?unknown")).is_none());
        assert_eq!(
            identifier_key(&bib_types::Identifier::new(
                IdKind::Arxiv,
                "HTTPS://ARXIV.ORG/PDF/2201.12345v2.pdf"
            ))
            .as_deref(),
            Some("arxiv:2201.12345")
        );
        assert!(
            identifier_key(&bib_types::Identifier::new(IdKind::Arxiv, "2201.12345v0")).is_none()
        );
        assert!(
            identifier_key(&bib_types::Identifier::new(
                IdKind::Arxiv,
                "not-an-arxiv-id"
            ))
            .is_none()
        );
    }
    fn record(
        source: &str,
        id: &str,
        doi: Option<&str>,
        pmid: Option<&str>,
        rank: usize,
    ) -> DiscoveryRecord {
        let mut article = Article::new(id, "A scientific result");
        if let Some(doi) = doi {
            article.identifiers.push(bib_types::Identifier::doi(doi));
        }
        if let Some(pmid) = pmid {
            article.identifiers.push(bib_types::Identifier::pmid(pmid));
        }
        DiscoveryRecord {
            source: source.into(),
            article,
            source_rank: rank,
            query_fingerprint: "query".into(),
            retrieved_at: "2026-10-27T00:00:00Z".into(),
            executed_parameters: Default::default(),
            query_translation: None,
        }
    }
    #[test]
    fn strong_id_bridge_unions_candidates_without_discarding_provenance() {
        let mut store = CandidateStore::default();
        store.ingest(vec![record("pubmed", "p", None, Some("101"), 1)]);
        let first = store.results(None, Default::default(), 0, 100).0[0]
            .id
            .clone();
        store.ingest(vec![record(
            "openalex",
            "o",
            Some("https://doi.org/10.1000/RESULT"),
            None,
            5,
        )]);
        assert_eq!(store.len(), 2);
        let delta = store.ingest(vec![record(
            "s2",
            "s",
            Some("10.1000/result"),
            Some("101"),
            2,
        )]);
        assert_eq!(store.len(), 1);
        assert_eq!(delta.removed_ids.len(), 1);
        let candidate = store.candidate(&first).unwrap();
        assert_eq!(candidate.records.len(), 3);
        assert_eq!(candidate.id, first);
        assert_eq!(candidate.article.id, "p");
        assert!((candidate.rrf_score - (1.0 / 61.0 + 1.0 / 65.0 + 1.0 / 62.0)).abs() < 1e-9);
        store.ingest(vec![record("pubmed", "p", None, Some("101"), 10)]);
        assert_eq!(store.candidate(&first).unwrap().records.len(), 3);
        assert_eq!(store.candidate(&delta.removed_ids[0]).unwrap().id, first);
    }
    #[test]
    fn conflicting_dois_and_title_similarity_never_auto_merge() {
        let mut store = CandidateStore::default();
        store.ingest(vec![
            record("pubmed", "a", Some("10.1000/first"), Some("101"), 1),
            record("openalex", "b", Some("10.1000/second"), Some("101"), 1),
        ]);
        assert_eq!(store.len(), 2);
        let results = store.results(None, Default::default(), 0, 10).0;
        assert_eq!(results[0].related_versions.len(), 1);
        assert_eq!(results[0].possible_duplicates.len(), 1);
        store.ingest(vec![record("s2", "bridge", None, Some("101"), 1)]);
        assert_eq!(
            store.len(),
            3,
            "ambiguous bridge must not select one DOI by arrival order"
        );
        let mut weak = CandidateStore::default();
        weak.ingest(vec![
            record("pubmed", "a", None, None, 1),
            record("openalex", "b", None, None, 1),
        ]);
        assert_eq!(weak.len(), 2);
    }
    #[test]
    fn conflicting_fields_remain_visible_and_source_ranks_do_not_replace_primary() {
        let mut store = CandidateStore::default();
        let first = record("pubmed", "a", Some("10.1000/result"), None, 2);
        let mut second = record("openalex", "b", Some("10.1000/result"), None, 1);
        second.article.title = "Different title".into();
        second.article.year = Some(2024);
        store.ingest(vec![first, second]);
        let result = store
            .results(Some("openalex"), DiscoveryResultOrder::SourceRank, 0, 10)
            .0;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].article.title, "Different title");
        assert_eq!(
            store.results(None, Default::default(), 0, 10).0[0]
                .article
                .title,
            "A scientific result"
        );
        assert!(result[0].conflicts.iter().any(|c| c.field == "title"));
        assert_eq!(result[0].records.len(), 2);
    }
    #[test]
    fn preprint_and_published_article_share_a_version_link_instead_of_a_row() {
        let mut preprint = record("arxiv", "preprint", None, None, 1);
        preprint.article.source = ArticleSource::Arxiv;
        preprint
            .article
            .identifiers
            .push(bib_types::Identifier::new(IdKind::Arxiv, "2201.12345v2"));
        let mut published = record("crossref", "published", None, None, 1);
        published
            .article
            .identifiers
            .push(bib_types::Identifier::new(
                IdKind::Arxiv,
                "https://arxiv.org/abs/2201.12345v1",
            ));
        for publication_type in [
            "Journal Article",
            "journal-article",
            "JournalArticle",
            "journal_article",
        ] {
            published.article.pub_types = vec![publication_type.into()];
            let mut store = CandidateStore::default();
            store.ingest(vec![preprint.clone(), published.clone()]);
            let rows = store.results(None, Default::default(), 0, 10).0;
            assert_eq!(rows.len(), 2, "publication type {publication_type}");
            assert_eq!(rows[0].related_versions[0].candidate_id, rows[1].id);
        }
    }
}
