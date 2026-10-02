//! Evidence-channel payload: typed citations flowing on DAG file edges.
//!
//! Contract: a [`dag_core`] `FileRef` whose `format == [`FORMAT`]` carries the
//! UTF-8 JSON encoding of [`EvidenceSet`]. Producers must attach a sha256
//! content hash to the `FileRef` fingerprint so downstream nodes get
//! content-addressed incremental identity.
//!
//! This module stays pure data + pure logic (no I/O) per the crate charter;
//! reading and writing artifact bytes lives in `bib-base::nodes`.

use serde::{Deserialize, Serialize};

use crate::convert::normalize_doi;
use crate::types::{Article, IdKind};

/// Format label carried on the `FileRef` and enforced by DAG port format
/// gates (wiring-time, dispatch-time, and output self-check).
pub const FORMAT: &str = "evidence";

/// Payload schema version. Writers always emit [`SCHEMA_VERSION`]; readers
/// fail closed on anything newer rather than guessing at unknown semantics.
pub const SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// EvidenceSet / EvidenceRecord
// ---------------------------------------------------------------------------

/// An ordered collection of evidence records.
///
/// Record order is part of the payload: merge preserves input port order and
/// first-seen order through deduplication, so two runs of the same DAG
/// produce byte-identical files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceSet {
    /// Payload schema version; writers always emit [`SCHEMA_VERSION`].
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Records in first-seen order.
    #[serde(default)]
    pub records: Vec<EvidenceRecord>,
}

impl Default for EvidenceSet {
    /// In-memory construction always carries the current schema version —
    /// a derived `Default` would emit `schema_version: 0` and produce
    /// non-conforming artifacts.
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            records: Vec::new(),
        }
    }
}

/// One piece of evidence: a bibliographic record plus channel metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    /// The bibliographic record (the evidence itself).
    pub citation: Article,
    /// Why this record is relevant (agent-supplied, optional).
    #[serde(default)]
    pub note: Option<String>,
    /// Which literature source produced the record (e.g. `"pubmed"`).
    #[serde(default)]
    pub origin: Option<String>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl EvidenceSet {
    /// Serialize to pretty-printed JSON bytes (evidence files are audit
    /// artifacts; deterministic pretty printing keeps them human-readable).
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec_pretty(self).map_err(|e| format!("failed to serialize evidence set: {e}"))
    }

    /// Parse fail-closed: unknown future schema versions are rejected by
    /// version number rather than guessed at. Older payloads (fields with
    /// `#[serde(default)]`) stay forward-compatible.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let set: Self = serde_json::from_slice(bytes)
            .map_err(|e| format!("invalid evidence payload: {e}"))?;
        if set.schema_version > SCHEMA_VERSION {
            return Err(format!(
                "evidence payload schema_version {} is newer than supported {}; \
                 regenerate the artifact with a current engine",
                set.schema_version, SCHEMA_VERSION
            ));
        }
        Ok(set)
    }

    /// Deduplicate by shared identifiers: two records are the same evidence
    /// when any of their normalized identifiers (DOI normalized and
    /// lowercased first, then PMID, arXiv, S2, OpenAlex, …) collide.
    /// Records with no identifiers fall back to the fuzzy
    /// [`cite_key`] layer.
    ///
    /// First occurrence wins for citation fields and `origin`; a missing
    /// `note` is filled from the first duplicate that has one. Returns the
    /// number of records removed.
    ///
    /// Known limitation: the same paper arriving once with only a PMID and
    /// once with only a DOI (no shared identifier) is kept twice — resolving
    /// that requires a cross-source lookup, which the merge node
    /// intentionally does not perform.
    pub fn dedup_in_place(&mut self) -> usize {
        let before = self.records.len();
        let mut kept: Vec<EvidenceRecord> = Vec::with_capacity(before);
        // key -> index into `kept`
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        for record in self.records.drain(..) {
            let keys = dedup_keys(&record);
            let mut existing: Option<usize> = None;
            for key in &keys {
                if let Some(&idx) = seen.get(key) {
                    existing = Some(idx);
                    break;
                }
            }
            match existing {
                Some(idx) => {
                    if kept[idx].note.is_none() {
                        if let Some(note) = record.note {
                            kept[idx].note = Some(note);
                        }
                    }
                }
                None => {
                    for key in keys {
                        seen.insert(key, kept.len());
                    }
                    kept.push(record);
                }
            }
        }

        let removed = before - kept.len();
        self.records = kept;
        removed
    }
}

// ---------------------------------------------------------------------------
// Dedup keys
// ---------------------------------------------------------------------------

/// All dedup keys for one record. A record with no identifiers yields the
/// single fuzzy [`cite_key`] key.
fn dedup_keys(record: &EvidenceRecord) -> Vec<String> {
    let mut keys: Vec<String> = record
        .citation
        .identifiers
        .iter()
        .map(|id| identifier_key(id.kind, &id.value))
        .collect();
    if keys.is_empty() {
        keys.push(format!("fuzzy:{}", cite_key(&record.citation)));
    }
    keys
}

/// Normalize one identifier into a dedup key. DOIs are stripped of URL
/// prefixes and lowercased (sources disagree on casing and prefix); other
/// identifiers are trimmed and lowercased.
fn identifier_key(kind: IdKind, value: &str) -> String {
    let normalized = if kind == IdKind::Doi {
        normalize_doi(value).to_lowercase()
    } else {
        value.trim().to_lowercase()
    };
    format!("{}:{normalized}", kind.as_str())
}

// ---------------------------------------------------------------------------
// Cite key
// ---------------------------------------------------------------------------

/// Build a cite key from first author's last name + year + first title word.
///
/// Moved from `bib-base::export` so the evidence payload layer can use it as
/// the fuzzy dedup fallback without a reverse dependency; `bib-base`
/// delegates to this implementation.
pub fn cite_key(article: &Article) -> String {
    let author_part = article
        .authors
        .first()
        .map(|a| {
            a.last_name
                .to_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        })
        .filter(|part| !part.is_empty())
        .unwrap_or_else(|| "anon".into());
    let year_part = article
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "nd".into());
    let title_part = article
        .title
        .split_whitespace()
        .next()
        .map(|w| {
            // Keep only alphanumeric characters so trailing punctuation
            // (e.g. "Tutorial:" → "tutorial") doesn't leak into the key.
            w.to_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| "untitled".into());
    format!("{author_part}{year_part}{title_part}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Author, Identifier};

    fn article(id: &str, title: &str) -> Article {
        Article::new(id, title)
    }

    fn record(id: &str, title: &str) -> EvidenceRecord {
        EvidenceRecord {
            citation: article(id, title),
            note: None,
            origin: None,
        }
    }

    fn with_author(mut a: Article, last_name: &str) -> Article {
        a.authors.push(Author {
            last_name: last_name.into(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        a
    }

    #[test]
    fn cite_key_matches_migrated_behaviour() {
        let mut a = with_author(article("x", "A breakthrough"), "Smith");
        a.year = Some(2024);
        a.identifiers.push(Identifier::doi("10.1000/test"));
        assert_eq!(cite_key(&a), "smith2024a");

        let mut no_author = article("x", "Hello World");
        no_author.year = Some(2020);
        assert_eq!(cite_key(&no_author), "anon2020hello");

        let mut titled = with_author(article("x", "Tutorial: A Guide to GWAS"), "Choi");
        titled.year = Some(2020);
        assert_eq!(cite_key(&titled), "choi2020tutorial");
    }

    #[test]
    fn dedup_merges_on_normalized_doi() {
        let mut r1 = record("1", "Same paper");
        r1.citation = with_author(r1.citation, "Smith");
        r1.citation.identifiers.push(Identifier::doi("10.1000/test"));

        let mut r2 = record("2", "Same paper, URL-prefixed DOI");
        r2.citation = with_author(r2.citation, "Smith");
        r2.citation
            .identifiers
            .push(Identifier::doi("https://doi.org/10.1000/TEST"));

        let mut r3 = record("3", "Different paper");
        r3.citation
            .identifiers
            .push(Identifier::doi("10.1000/other"));

        let mut set = EvidenceSet {
            records: vec![r1, r2, r3],
            ..Default::default()
        };
        assert_eq!(set.dedup_in_place(), 1);
        assert_eq!(set.records.len(), 2);
        // First occurrence wins.
        assert_eq!(set.records[0].citation.title, "Same paper");
        assert_eq!(set.records[1].citation.title, "Different paper");
    }

    #[test]
    fn dedup_merges_on_any_identifier_kind() {
        let mut r1 = record("1", "PubMed view");
        r1.citation.identifiers.push(Identifier::pmid("12345"));
        r1.citation
            .identifiers
            .push(Identifier::new(IdKind::S2, "AbC123"));

        let mut r2 = record("2", "S2 view, different title string");
        r2.citation
            .identifiers
            .push(Identifier::new(IdKind::S2, "abc123"));

        let mut set = EvidenceSet {
            records: vec![r1, r2],
            ..Default::default()
        };
        assert_eq!(set.dedup_in_place(), 1);
        assert_eq!(set.records.len(), 1);
        assert_eq!(set.records[0].citation.title, "PubMed view");
    }

    #[test]
    fn dedup_fuzzy_fallback_without_identifiers() {
        let mut r1 = record("1", "Tutorial: A Guide");
        r1.citation = with_author(r1.citation, "Choi");
        r1.citation.year = Some(2020);

        let mut r2 = record("2", "Tutorial: A longer subtitle");
        r2.citation = with_author(r2.citation, "Choi");
        r2.citation.year = Some(2020);

        let mut set = EvidenceSet {
            records: vec![r1, r2],
            ..Default::default()
        };
        assert_eq!(set.dedup_in_place(), 1);
    }

    #[test]
    fn dedup_keeps_disjoint_identifiers() {
        // Same paper via PMID-only and DOI-only: known limitation, kept twice.
        let mut r1 = record("1", "PMID view");
        r1.citation.identifiers.push(Identifier::pmid("12345"));
        let mut r2 = record("2", "DOI view");
        r2.citation.identifiers.push(Identifier::doi("10.1000/x"));

        let mut set = EvidenceSet {
            records: vec![r1, r2],
            ..Default::default()
        };
        assert_eq!(set.dedup_in_place(), 0);
        assert_eq!(set.records.len(), 2);
    }

    #[test]
    fn dedup_fills_missing_note_from_duplicate() {
        let mut r1 = record("1", "First, no note");
        r1.citation.identifiers.push(Identifier::doi("10.1000/t"));
        let mut r2 = record("2", "Second, has note");
        r2.citation.identifiers.push(Identifier::doi("10.1000/t"));
        r2.note = Some("why relevant".into());
        let mut r3 = record("3", "Third, later note ignored");
        r3.citation.identifiers.push(Identifier::doi("10.1000/t"));
        r3.note = Some("later".into());

        let mut set = EvidenceSet {
            records: vec![r1, r2, r3],
            ..Default::default()
        };
        assert_eq!(set.dedup_in_place(), 2);
        assert_eq!(set.records.len(), 1);
        assert_eq!(set.records[0].note.as_deref(), Some("why relevant"));
    }

    #[test]
    fn parse_rejects_newer_schema_version() {
        let json = r#"{"schema_version": 2, "records": []}"#;
        let err = EvidenceSet::parse(json.as_bytes()).unwrap_err();
        assert!(err.contains("schema_version 2"), "unexpected error: {err}");
    }

    #[test]
    fn parse_accepts_missing_schema_version() {
        let set = EvidenceSet::parse(br#"{"records": []}"#).unwrap();
        assert_eq!(set.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn serde_roundtrip_preserves_order_and_fields() {
        let mut r1 = record("1", "One");
        r1.citation.identifiers.push(Identifier::doi("10.1000/a"));
        r1.note = Some("note one".into());
        r1.origin = Some("pubmed".into());
        let r2 = record("2", "Two");

        let set = EvidenceSet {
            records: vec![r1, r2],
            ..Default::default()
        };
        let bytes = set.to_bytes().unwrap();
        let back = EvidenceSet::parse(&bytes).unwrap();
        assert_eq!(set, back);
    }

    #[test]
    fn writer_emits_current_schema_version() {
        let set = EvidenceSet::default();
        let bytes = set.to_bytes().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains(&format!("\"schema_version\": {SCHEMA_VERSION}")));
    }
}
