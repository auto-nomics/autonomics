use serde::{Deserialize, Serialize};

/// A normalized absolute path used as the durable identity of a knowledge node.
///
/// Paths, rather than a closed type enum, define the knowledge taxonomy. The
/// consolidation model chooses and evolves paths; the repository normalizes
/// them and enforces global uniqueness.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct KnowledgePath(Vec<String>);

impl KnowledgePath {
    #[must_use]
    pub fn new(segments: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let segments = segments.into_iter().map(Into::into).collect::<Vec<_>>();
        Self::from_segments(segments)
            .unwrap_or_else(|| Self(vec!["uncategorized".into(), "knowledge".into()]))
    }

    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.0
    }

    #[must_use]
    pub fn as_str(&self) -> String {
        format!("/{}", self.0.join("/"))
    }

    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.0.len() <= 1 {
            return None;
        }
        self.0.split_last().map(|(_, parent)| Self(parent.to_vec()))
    }

    #[must_use]
    pub fn leaf(&self) -> &str {
        self.0.last().map(String::as_str).unwrap_or_default()
    }

    #[must_use]
    pub fn from_segments(segments: impl IntoIterator<Item = impl Into<String>>) -> Option<Self> {
        let segments = segments.into_iter().map(Into::into).collect::<Vec<_>>();
        if segments.is_empty() || segments.len() > 8 || segments.iter().any(String::is_empty) {
            None
        } else {
            Some(Self(segments))
        }
    }

    /// Normalize an LLM-proposed path without imposing a fixed taxonomy.
    #[must_use]
    pub fn normalize(raw: &str) -> Option<Self> {
        let mut segments = Vec::new();
        for segment in raw.split('/') {
            let mut normalized = String::new();
            let mut last_separator = false;
            for ch in segment.trim().chars() {
                if ch.is_whitespace() {
                    if !last_separator && !normalized.is_empty() {
                        normalized.push('-');
                        last_separator = true;
                    }
                } else if ch.is_control() || matches!(ch, '?' | '#' | '%') {
                    return None;
                } else {
                    for lower in ch.to_lowercase() {
                        normalized.push(lower);
                    }
                    last_separator = false;
                }
            }
            let normalized = normalized.trim_matches('-');
            if normalized.is_empty() {
                continue;
            }
            if normalized.len() > 96 || normalized.chars().count() < 2 {
                return None;
            }
            segments.push(normalized.to_string());
        }
        if segments.len() < 2 || segments.len() > 8 {
            None
        } else {
            Some(Self(segments))
        }
    }
}

impl TryFrom<String> for KnowledgePath {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::normalize(&value).ok_or_else(|| format!("invalid knowledge path: {value}"))
    }
}

impl From<KnowledgePath> for String {
    fn from(value: KnowledgePath) -> Self {
        value.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_llm_paths_without_fixing_taxonomy() {
        let path = KnowledgePath::normalize(" /Statistics/Linear Regression/Model Checks/ ")
            .expect("valid path");
        assert_eq!(path.as_str(), "/statistics/linear-regression/model-checks");
        assert_eq!(path.leaf(), "model-checks");
        assert_eq!(
            path.parent().expect("has parent").as_str(),
            "/statistics/linear-regression"
        );
    }

    #[test]
    fn rejects_paths_that_cannot_form_tree_identity() {
        assert!(KnowledgePath::normalize("/x").is_none());
        assert!(KnowledgePath::normalize("/too/many#characters").is_none());
        assert_eq!(
            KnowledgePath::new(Vec::<String>::new()).as_str(),
            "/uncategorized/knowledge"
        );
    }
}
