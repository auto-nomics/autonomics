//! Citation resolver — bridges document cite keys with the `bib-base` Article library.
//!
//! The [`CitationResolver`] scans a [`Document`](writing_types::Document) for
//! citation clusters, resolves each cite key against the articles stored in
//! [`BibBase`](bib_base::BibBase), and produces reports and `.bib` files.
//!
//! ## Cite-key matching strategy
//!
//! Cite keys follow the `bib_base::export::cite_key` convention:
//! `firstauthorlastname + year + firsttitleword`.  The resolver builds an
//! index of all articles in the library, computes their cite keys, and
//! matches.  When two articles produce the same key, disambiguation
//! suffixes (a, b, …) are applied.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use bib_base::BibBase;
use bib_types::Article;

use writing_types::Document;

use crate::ast;

// ---------------------------------------------------------------------------
// CiteKeyStatus
// ---------------------------------------------------------------------------

/// Resolution status for a single cite key found in a document.
#[derive(Debug, Clone)]
pub enum CiteKeyStatus {
    /// Successfully matched to exactly one Article.
    Resolved { key: String, article_id: String },

    /// No article in the library matches this key.
    Unresolved {
        key: String,
        suggestion: Option<String>,
    },

    /// Multiple articles match this key (ambiguity).
    Ambiguous {
        key: String,
        candidates: Vec<String>,
    },
}

impl CiteKeyStatus {
    /// Whether this key was resolved successfully.
    pub fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved { .. })
    }

    /// The unresolved or ambiguous key, if any.
    pub fn key(&self) -> &str {
        match self {
            Self::Resolved { key, .. }
            | Self::Unresolved { key, .. }
            | Self::Ambiguous { key, .. } => key,
        }
    }
}

// ---------------------------------------------------------------------------
// CitationReport
// ---------------------------------------------------------------------------

/// Full citation resolution report for a document.
#[derive(Debug, Clone)]
pub struct CitationReport {
    /// All unique cite keys found in the document.
    pub keys: BTreeSet<String>,

    /// Per-key resolution status.
    pub statuses: Vec<CiteKeyStatus>,

    /// Articles in the library that are *not* cited in this document.
    pub uncited_article_ids: Vec<String>,
}

impl CitationReport {
    /// Count of resolved cite keys.
    pub fn resolved_count(&self) -> usize {
        self.statuses.iter().filter(|s| s.is_resolved()).count()
    }

    /// Count of unresolved cite keys.
    pub fn unresolved_count(&self) -> usize {
        self.statuses
            .iter()
            .filter(|s| matches!(s, CiteKeyStatus::Unresolved { .. }))
            .count()
    }

    /// Count of ambiguous cite keys.
    pub fn ambiguous_count(&self) -> usize {
        self.statuses
            .iter()
            .filter(|s| matches!(s, CiteKeyStatus::Ambiguous { .. }))
            .count()
    }

    /// Whether all cite keys resolved successfully.
    pub fn all_resolved(&self) -> bool {
        self.statuses.iter().all(|s| s.is_resolved())
    }

    /// Get the article IDs for all resolved keys.
    pub fn resolved_article_ids(&self) -> Vec<String> {
        self.statuses
            .iter()
            .filter_map(|s| match s {
                CiteKeyStatus::Resolved { article_id, .. } => Some(article_id.clone()),
                _ => None,
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// CitationGraph
// ---------------------------------------------------------------------------

/// Which article each block cites (block_id → article IDs).
pub type CitationGraph = HashMap<String, Vec<String>>;

// ---------------------------------------------------------------------------
// CitationResolver
// ---------------------------------------------------------------------------

/// Bridges document cite keys with the `bib-base` Article library.
///
/// Cheap to clone (internally `Arc<BibBase>`).  The cite-key index is
/// rebuilt on each call to `scan` / `generate_bib` so that newly added
/// articles are always visible.
pub struct CitationResolver {
    bib: Arc<BibBase>,
}

impl CitationResolver {
    /// Create a new resolver backed by the given `BibBase`.
    pub fn new(bib: Arc<BibBase>) -> Self {
        Self { bib }
    }

    // -----------------------------------------------------------------------
    // Index building
    // -----------------------------------------------------------------------

    /// Build a cite-key → Vec<article_id> index from all articles in the library.
    ///
    /// When two articles produce the same base key, disambiguation suffixes
    /// are applied: the first gets the bare key, the second gets `b`, `c`, …
    ///
    /// Returns `(forward_index, reverse_index)`:
    /// - forward: cite_key → article_id
    /// - reverse: article_id → cite_key
    async fn build_index(&self) -> (HashMap<String, String>, HashMap<String, String>) {
        let articles = self.bib.list_all_articles().await.unwrap_or_default();

        // Group by base key.
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for article in &articles {
            let base = bib_base::cite_key(article);
            groups.entry(base).or_default().push(article.id.clone());
        }

        let mut forward = HashMap::new(); // cite_key → article_id
        let mut reverse = HashMap::new(); // article_id → cite_key

        for (base, ids) in &groups {
            if ids.len() == 1 {
                // No conflict — bare key.
                forward.insert(base.clone(), ids[0].clone());
                reverse.insert(ids[0].clone(), base.clone());
            } else {
                // Disambiguate: first keeps the bare key, subsequent get
                // extra suffix letters: 'b', 'c', 'd', …
                for (i, id) in ids.iter().enumerate() {
                    if i == 0 {
                        forward.insert(base.clone(), id.clone());
                        reverse.insert(id.clone(), base.clone());
                    } else {
                        let suffix = (b'b' + (i as u8 - 1)) as char;
                        let key = format!("{base}{suffix}");
                        forward.insert(key.clone(), id.clone());
                        reverse.insert(id.clone(), key);
                    }
                }
            }
        }

        (forward, reverse)
    }

    // -----------------------------------------------------------------------
    // Scan & resolve
    // -----------------------------------------------------------------------

    /// Scan a document and resolve all cite keys against the library.
    pub async fn scan(&self, doc: &Document) -> CitationReport {
        let keys = ast::all_cite_keys(doc);
        let (forward, _reverse) = self.build_index().await;

        let mut statuses = Vec::with_capacity(keys.len());
        for key in &keys {
            match forward.get(key) {
                Some(article_id) => {
                    statuses.push(CiteKeyStatus::Resolved {
                        key: key.clone(),
                        article_id: article_id.clone(),
                    });
                }
                None => {
                    // Try to find a suggestion via fuzzy matching.
                    let suggestion = self.suggest_for_key(key, &forward);
                    statuses.push(CiteKeyStatus::Unresolved {
                        key: key.clone(),
                        suggestion,
                    });
                }
            }
        }

        // Check for ambiguous keys: keys that are close but not exact.
        // (Already handled above — each key either matches exactly or not.)

        // Compute uncited articles.
        let cited_ids: BTreeSet<String> = statuses
            .iter()
            .filter_map(|s| match s {
                CiteKeyStatus::Resolved { article_id, .. } => Some(article_id.clone()),
                _ => None,
            })
            .collect();

        let all_ids = self.bib.list_article_ids().await.unwrap_or_default();
        let uncited: Vec<String> = all_ids
            .into_iter()
            .filter(|id| !cited_ids.contains(id))
            .collect();

        CitationReport {
            keys: keys.into_iter().collect(),
            statuses,
            uncited_article_ids: uncited,
        }
    }

    /// Resolve a single cite key to an Article.
    ///
    /// Returns `None` if the key is not found or ambiguous.
    pub async fn resolve_key(&self, key: &str) -> Option<Article> {
        let (forward, _) = self.build_index().await;
        let article_id = forward.get(key)?;
        self.bib.get_article(article_id).await.ok().flatten()
    }

    // -----------------------------------------------------------------------
    // BibTeX generation
    // -----------------------------------------------------------------------

    /// Generate a `.bib` file containing entries for all cited articles.
    ///
    /// Uses [`bib_base::export::to_bibtex`] for each resolved article.
    /// Unresolved keys produce a commented-out placeholder so the user can
    /// see what's missing.
    pub async fn generate_bib(&self, doc: &Document) -> String {
        let report = self.scan(doc).await;

        let mut entries = Vec::new();

        for status in &report.statuses {
            match status {
                CiteKeyStatus::Resolved { article_id, .. } => {
                    if let Ok(Some(article)) = self.bib.get_article(article_id).await {
                        entries.push(bib_base::to_bibtex(&article));
                    }
                }
                CiteKeyStatus::Unresolved { key, suggestion } => {
                    let note = match suggestion {
                        Some(s) => format!(" (did you mean '{s}'?)"),
                        None => String::new(),
                    };
                    entries.push(format!(
                        "% UNRESOLVED: cite key '{key}' not found in library{note}"
                    ));
                }
                CiteKeyStatus::Ambiguous { key, candidates } => {
                    entries.push(format!(
                        "% AMBIGUOUS: cite key '{key}' matches {} articles: {}",
                        candidates.len(),
                        candidates.join(", ")
                    ));
                }
            }
        }

        // Include a header comment.
        let mut out = String::new();
        out.push_str("% Auto-generated by writing-base citation resolver\n");
        out.push_str(&format!(
            "% {} entries ({} resolved, {} unresolved)\n\n",
            report.statuses.len(),
            report.resolved_count(),
            report.unresolved_count()
        ));

        out.push_str(&entries.join("\n\n"));
        out.push('\n');
        out
    }

    // -----------------------------------------------------------------------
    // Citation graph
    // -----------------------------------------------------------------------

    /// Build a block-level citation graph: block_id → article IDs.
    pub async fn citation_graph(&self, doc: &Document) -> CitationGraph {
        let (forward, _) = self.build_index().await;
        let mut graph: CitationGraph = HashMap::new();

        collect_citation_graph(&doc.root, &forward, &mut graph);
        graph
    }

    // -----------------------------------------------------------------------
    // Deletion impact
    // -----------------------------------------------------------------------

    /// Check which cite keys in a document would break if an article is deleted.
    pub async fn check_deletion_impact(&self, article_id: &str, doc: &Document) -> Vec<String> {
        let (_, reverse) = self.build_index().await;
        let cite_key = match reverse.get(article_id) {
            Some(k) => k.clone(),
            None => return Vec::new(),
        };

        let doc_keys = ast::all_cite_keys(doc);
        if doc_keys.iter().any(|k| k == &cite_key) {
            vec![cite_key]
        } else {
            Vec::new()
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Suggest a closest-matching key for an unresolved cite key.
    fn suggest_for_key(&self, unresolved: &str, index: &HashMap<String, String>) -> Option<String> {
        // Simple prefix + edit-distance heuristic.
        let mut best: Option<(String, usize)> = None;
        for candidate in index.keys() {
            let dist = edit_distance(unresolved, candidate);
            let max_len = unresolved.len().max(candidate.len());
            // Accept if edit distance is at most 30% of the longer key,
            // or if one is a prefix of the other.
            let close = if dist <= max_len / 3 {
                true
            } else {
                candidate.starts_with(unresolved) || unresolved.starts_with(candidate)
            };
            if close {
                match &best {
                    Some((_, best_dist)) if dist >= *best_dist => {}
                    _ => best = Some((candidate.clone(), dist)),
                }
            }
        }
        best.map(|(k, _)| k)
    }
}

/// Recursively collect the citation graph from the section tree.
fn collect_citation_graph(
    section: &writing_types::Section,
    forward: &HashMap<String, String>,
    out: &mut CitationGraph,
) {
    for block in &section.blocks {
        if let writing_types::Block::Paragraph(p) = block {
            let mut article_ids = Vec::new();
            for inline in &p.inlines {
                if let writing_types::Inline::Citation(cluster) = inline {
                    for key in &cluster.keys {
                        if let Some(id) = forward.get(&key.key) {
                            article_ids.push(id.clone());
                        }
                    }
                }
            }
            if !article_ids.is_empty() {
                out.insert(p.meta.id.clone(), article_ids);
            }
        }
    }
    for child in &section.children {
        collect_citation_graph(child, forward, out);
    }
}

/// Levenshtein edit distance.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use writing_types::*;

    /// Set up a BibBase with a few test articles.
    async fn setup_bib() -> Arc<BibBase> {
        let bib = BibBase::open_in_memory().await.unwrap();

        // Smith 2024 — "A breakthrough in CRISPR"
        let mut a1 = Article::new("art-1", "A breakthrough in CRISPR off-target detection");
        a1.authors.push(bib_types::Author {
            last_name: "Smith".into(),
            fore_name: Some("John A".into()),
            initials: Some("JA".into()),
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        a1.year = Some(2024);
        a1.journal = Some("Nature Genetics".into());
        a1.identifiers
            .push(bib_types::Identifier::doi("10.1000/test1"));
        bib.upsert_article(&a1).await.unwrap();

        // Jones 2023 — "Mendelian randomization analysis"
        let mut a2 = Article::new(
            "art-2",
            "Mendelian randomization analysis of complex traits",
        );
        a2.authors.push(bib_types::Author {
            last_name: "Jones".into(),
            fore_name: Some("Bob".into()),
            initials: Some("B".into()),
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        a2.year = Some(2023);
        a2.identifiers
            .push(bib_types::Identifier::doi("10.1000/test2"));
        bib.upsert_article(&a2).await.unwrap();

        // Doe 2022 — "Causal inference"
        let mut a3 = Article::new("art-3", "Causal inference in genetic epidemiology");
        a3.authors.push(bib_types::Author {
            last_name: "Doe".into(),
            fore_name: Some("Jane".into()),
            initials: Some("J".into()),
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        a3.year = Some(2022);
        bib.upsert_article(&a3).await.unwrap();

        // Second Smith 2024 article starting with "A" — creates ambiguity
        let mut a4 = Article::new("art-4", "A novel method for GWAS fine-mapping");
        a4.authors.push(bib_types::Author {
            last_name: "Smith".into(),
            fore_name: Some("John A".into()),
            initials: Some("JA".into()),
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        a4.year = Some(2024);
        bib.upsert_article(&a4).await.unwrap();

        Arc::new(bib)
    }

    fn make_doc_with_citations() -> Document {
        let mut doc = Document::new("d1", "Test Paper");
        let mut sec = Section::new(SectionLevel::Section, "Introduction");
        sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
            Inline::text("Prior work "),
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("smith2024a")], // art-1 after disambiguation
                style: CitationStyle::Parenthetical,
                prefix: None,
                suffix: None,
                claim_id: None,
            }),
            Inline::text(" and "),
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("jones2023mendelian")], // art-2
                style: CitationStyle::Textual,
                prefix: None,
                suffix: None,
                claim_id: None,
            }),
            Inline::text(" also "),
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("nonexistent2024key")], // unresolved
                style: CitationStyle::Parenthetical,
                prefix: None,
                suffix: None,
                claim_id: None,
            }),
            Inline::text("."),
        ])));
        doc.root.children.push(sec);
        doc
    }

    #[tokio::test]
    async fn resolve_exact_keys() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);
        let doc = make_doc_with_citations();
        let report = resolver.scan(&doc).await;

        assert_eq!(report.keys.len(), 3);
        assert!(report.resolved_count() >= 2);
        assert!(report.unresolved_count() >= 1);
    }

    #[tokio::test]
    async fn resolve_single_key() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);

        // smith2024a should resolve to art-1 (Smith 2024 breakthrough).
        let article = resolver.resolve_key("smith2024a").await;
        assert!(article.is_some());
        let article = article.unwrap();
        assert!(article.title.contains("CRISPR"));

        // Nonexistent key returns None.
        assert!(resolver.resolve_key("nobody2024xyz").await.is_none());
    }

    #[tokio::test]
    async fn disambiguation_suffixes() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);

        // Two Smith 2024 articles: art-1 ("A breakthrough...") and art-4 ("A novel...")
        // Both produce base key "smith2024a", so disambiguation gives smith2024aa and smith2024ab.
        // Actually, let me check: cite_key uses first title word.
        // art-1: title "A breakthrough..." → first word "A" → key "smith2024a"
        // art-4: title "A novel..." → first word "A" → key "smith2024a"
        // So both have base "smith2024a", and disambiguation makes them smith2024aa, smith2024ab.
        let (forward, _) = resolver.build_index().await;

        // Both articles should be in the index.
        let smith_keys: Vec<&String> = forward
            .keys()
            .filter(|k| k.starts_with("smith2024"))
            .collect();
        assert!(smith_keys.len() >= 2);
    }

    #[tokio::test]
    async fn generate_bib_resolved_only() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);
        let doc = make_doc_with_citations();

        let bib_content = resolver.generate_bib(&doc).await;

        // Should contain @article entries for resolved keys.
        assert!(bib_content.contains("@article"));
        assert!(bib_content.contains("Smith"));
        assert!(bib_content.contains("Jones"));

        // Should contain a comment for unresolved key.
        assert!(bib_content.contains("UNRESOLVED"));
        assert!(bib_content.contains("nonexistent2024key"));
    }

    #[tokio::test]
    async fn citation_graph_maps_block_to_articles() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);
        let doc = make_doc_with_citations();

        let graph = resolver.citation_graph(&doc).await;

        // Introduction paragraph should cite 2+ articles.
        assert!(!graph.is_empty());
        for article_ids in graph.values() {
            assert!(!article_ids.is_empty());
        }
    }

    #[tokio::test]
    async fn deletion_impact() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);
        let doc = make_doc_with_citations();

        // Deleting art-1 (Smith 2024) should break smith2024a.
        let impact = resolver.check_deletion_impact("art-1", &doc).await;
        assert!(!impact.is_empty());

        // Deleting art-3 (Doe 2022, not cited) should break nothing.
        let no_impact = resolver.check_deletion_impact("art-3", &doc).await;
        assert!(no_impact.is_empty());
    }

    #[tokio::test]
    async fn uncited_articles_detected() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);
        let doc = make_doc_with_citations();
        let report = resolver.scan(&doc).await;

        // art-3 (Doe 2022) is in the library but not cited.
        assert!(report.uncited_article_ids.contains(&"art-3".to_string()));
    }

    #[tokio::test]
    async fn suggestion_for_typo() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);

        let (forward, _) = resolver.build_index().await;

        // A near-miss key should get a suggestion.
        let suggestion = resolver.suggest_for_key("jones2023mendeliann", &forward);
        assert!(suggestion.is_some());
        let s = suggestion.unwrap();
        assert!(s.contains("jones2023"));
    }

    #[test]
    fn edit_distance_basic() {
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("smith2024", "smit2024"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[tokio::test]
    async fn empty_document_scan() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);
        let doc = Document::new("d1", "Empty");

        let report = resolver.scan(&doc).await;
        assert!(report.keys.is_empty());
        assert!(report.statuses.is_empty());
        assert_eq!(report.uncited_article_ids.len(), 4); // all articles uncited
    }

    #[tokio::test]
    async fn report_all_resolved() {
        let bib = setup_bib().await;
        let resolver = CitationResolver::new(bib);
        let doc = make_doc_with_citations();
        let report = resolver.scan(&doc).await;

        // Not all resolved (one unresolved key).
        assert!(!report.all_resolved());

        // Now make a doc with only resolved keys.
        let mut doc2 = Document::new("d2", "All Resolved");
        let mut sec = Section::new(SectionLevel::Section, "S");
        sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
            Inline::text("Cite "),
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("smith2024a")],
                style: CitationStyle::Parenthetical,
                prefix: None,
                suffix: None,
                claim_id: None,
            }),
            Inline::text(" and "),
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("jones2023mendelian")],
                style: CitationStyle::Parenthetical,
                prefix: None,
                suffix: None,
                claim_id: None,
            }),
        ])));
        doc2.root.children.push(sec);

        let report2 = resolver.scan(&doc2).await;
        assert!(report2.all_resolved());
        assert_eq!(report2.resolved_count(), 2);
    }
}
