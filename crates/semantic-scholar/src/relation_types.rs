//! Nullable graph entries retain the raw upstream window, including papers
//! Semantic Scholar could not resolve. Recommendations are separate relations.
use crate::Paper;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperRelation {
    References,
    Citations,
}
impl PaperRelation {
    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::References => "references",
            Self::Citations => "citations",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecommendationPool {
    #[serde(rename = "recent")]
    Recent,
    #[serde(rename = "all-cs")]
    AllComputerScience,
}
impl RecommendationPool {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::AllComputerScience => "all-cs",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum RelationPaper {
    Resolved(Box<Paper>),
    /// Metadata with an absent, null or empty paperId, kept for review.
    Unresolved(Map<String, Value>),
}
impl RelationPaper {
    pub fn resolved(&self) -> Option<&Paper> {
        match self {
            Self::Resolved(paper) => Some(paper),
            Self::Unresolved(_) => None,
        }
    }
}
impl<'de> Deserialize<'de> for RelationPaper {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let object = Map::<String, Value>::deserialize(deserializer)?;
        match object.get("paperId") {
            None | Some(Value::Null) => Ok(Self::Unresolved(object)),
            Some(Value::String(id)) if id.trim().is_empty() => Ok(Self::Unresolved(object)),
            Some(Value::String(_)) => serde_json::from_value(Value::Object(object))
                .map(|paper| Self::Resolved(Box::new(paper)))
                .map_err(serde::de::Error::custom),
            _ => Err(serde::de::Error::custom("paperId must be a string or null")),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RelationRecord {
    pub contexts: Option<Vec<String>>,
    pub intents: Option<Vec<String>>,
    #[serde(rename = "isInfluential")]
    pub is_influential: Option<bool>,
    #[serde(
        default,
        rename = "citedPaper",
        skip_serializing_if = "Option::is_none"
    )]
    pub cited_paper: Option<RelationPaper>,
    #[serde(
        default,
        rename = "citingPaper",
        skip_serializing_if = "Option::is_none"
    )]
    pub citing_paper: Option<RelationPaper>,
}
impl RelationRecord {
    pub fn paper(&self, relation: PaperRelation) -> Option<&RelationPaper> {
        match relation {
            PaperRelation::References => self.cited_paper.as_ref(),
            PaperRelation::Citations => self.citing_paper.as_ref(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RelationPage {
    pub offset: u32,
    pub next: Option<u32>,
    pub data: Vec<RelationRecord>,
}
