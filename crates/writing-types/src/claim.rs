//! Claim — structured annotation for evidence-backed assertions.
//!
//! A [`Claim`] marks a span of text within a paragraph as a statement that
//! requires evidentiary support.  Each claim carries [`EvidenceLink`]s that
//! point to citations, data results, figures, or tables.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// TextSpan
// ---------------------------------------------------------------------------

/// A character span within a block's plain-text rendering.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TextSpan {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl TextSpan {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Length of the span.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether the span is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Claim
// ---------------------------------------------------------------------------

/// A structured claim — a span of text that asserts something requiring
/// evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub block_id: String,
    /// Character span within the block's inline content.
    pub span: TextSpan,
    #[serde(default)]
    pub claim_type: ClaimType,
    #[serde(default)]
    pub evidence: Vec<EvidenceLink>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub confidence: Option<Confidence>,
}

// ---------------------------------------------------------------------------
// ClaimType
// ---------------------------------------------------------------------------

/// The epistemic category of a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimType {
    /// A testable hypothesis.
    Hypothesis,
    /// A finding / outcome of analysis.
    Result,
    /// Description of methodology.
    Method,
    /// Established background fact.
    Background,
    /// Acknowledged limitation.
    Limitation,
    /// Proposed future direction.
    FutureWork,
    /// A formal definition.
    Definition,
    /// An unstated assumption.
    Assumption,
}

impl Default for ClaimType {
    fn default() -> Self {
        Self::Background
    }
}

// ---------------------------------------------------------------------------
// EvidenceLink
// ---------------------------------------------------------------------------

/// A link from a claim to its supporting evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceLink {
    pub kind: EvidenceType,
    pub target: EvidenceTarget,
    #[serde(default)]
    pub note: Option<String>,
}

/// The type of evidence backing a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    /// Supported by a citation.
    Citation,
    /// Supported by a data analysis result.
    DataResult,
    /// Supported by a figure.
    Figure,
    /// Supported by a table.
    Table,
    /// Supported by an external resource.
    ExternalUrl,
}

/// The target of an evidence link.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EvidenceTarget {
    /// Points to a `bib-base` Article via cite key.
    Article { cite_key: String },
    /// Points to a DAG node output.
    DagOutput {
        dag_id: String,
        node_id: String,
        #[serde(default)]
        port: Option<String>,
    },
    /// Points to a block within this document.
    BlockRef { block_id: String },
    /// An external URL.
    Url { url: String },
}

// ---------------------------------------------------------------------------
// Confidence
// ---------------------------------------------------------------------------

/// Author's confidence in a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Strongly supported by evidence.
    High,
    /// Moderately supported.
    Medium,
    /// Weakly supported or speculative.
    Low,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_span_len() {
        let s = TextSpan::new(5, 15);
        assert_eq!(s.len(), 10);
        assert!(!s.is_empty());

        let empty = TextSpan::new(3, 3);
        assert!(empty.is_empty());
    }

    #[test]
    fn claim_round_trip() {
        let claim = Claim {
            id: "claim_1".into(),
            block_id: "blk_42".into(),
            span: TextSpan::new(0, 20),
            claim_type: ClaimType::Result,
            evidence: vec![
                EvidenceLink {
                    kind: EvidenceType::Citation,
                    target: EvidenceTarget::Article {
                        cite_key: "smith2024".into(),
                    },
                    note: Some("Key finding replicated".into()),
                },
                EvidenceLink {
                    kind: EvidenceType::DataResult,
                    target: EvidenceTarget::DagOutput {
                        dag_id: "mr_analysis".into(),
                        node_id: "ivw".into(),
                        port: None,
                    },
                    note: None,
                },
            ],
            tags: vec!["primary".into()],
            confidence: Some(Confidence::High),
        };

        let json = serde_json::to_string(&claim).unwrap();
        let back: Claim = serde_json::from_str(&json).unwrap();
        assert_eq!(back.claim_type, ClaimType::Result);
        assert_eq!(back.evidence.len(), 2);
        assert_eq!(back.confidence, Some(Confidence::High));
    }

    #[test]
    fn evidence_target_serialization() {
        let t = EvidenceTarget::Article {
            cite_key: "doe2021".into(),
        };
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["type"], "article");
        assert_eq!(json["cite_key"], "doe2021");

        let t2 = EvidenceTarget::Url {
            url: "https://example.com".into(),
        };
        let json2 = serde_json::to_value(&t2).unwrap();
        assert_eq!(json2["type"], "url");
    }
}
