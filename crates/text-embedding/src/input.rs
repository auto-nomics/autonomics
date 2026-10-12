//! The record embedders consume: an id and the text to embed.

/// One text to embed: a caller-chosen id and the text itself.
///
/// The id flows through unchanged so callers can key the returned
/// vectors back to their records without positional bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingInput {
    pub id: String,
    pub text: String,
}

impl EmbeddingInput {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_into_strings() {
        let input = EmbeddingInput::new("obs-1", String::from("body text"));
        assert_eq!(input.id, "obs-1");
        assert_eq!(input.text, "body text");
    }
}
