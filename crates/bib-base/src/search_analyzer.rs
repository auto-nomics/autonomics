//! Shared lexical analysis for the index, query clauses, and original offsets.
//! Only Han runs use Jieba: Latin/Greek scientific words keep Unicode word
//! boundaries and case folding. Dictionary/model versions are Cargo-locked.
use jieba_rs::Jieba;
use std::sync::LazyLock;
use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

static JIEBA: LazyLock<Jieba> = LazyLock::new(Jieba::new);

pub(crate) fn is_han(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
}

/// Non-overlapping position units: one Han character or one Unicode word.
/// This lets phrases cross dictionary boundaries without losing adjacency.
pub(crate) fn position_tokens(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut next_position = 0;
    for (position, ch) in text.char_indices() {
        if !ch.is_alphanumeric() || is_han(ch) {
            if let Some(start) = start.take() {
                tokens.push(Token {
                    offset_from: start,
                    offset_to: position,
                    position: next_position,
                    text: text[start..position].to_lowercase(),
                    position_length: 1,
                });
                next_position += 1;
            }
            if ch == '\u{1e}' {
                next_position += 1;
            }
            if is_han(ch) {
                tokens.push(Token {
                    offset_from: position,
                    offset_to: position + ch.len_utf8(),
                    position: next_position,
                    text: ch.to_string(),
                    position_length: 1,
                });
                next_position += 1;
            }
        } else if start.is_none() {
            start = Some(position);
        }
    }
    if let Some(start) = start {
        tokens.push(Token {
            offset_from: start,
            offset_to: text.len(),
            position: next_position,
            text: text[start..].to_lowercase(),
            position_length: 1,
        });
    }
    tokens
}

pub(crate) fn analyze(text: &str) -> Vec<Token> {
    let units = position_tokens(text);
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < units.len() {
        let unit = &units[at];
        if !unit.text.chars().all(is_han) {
            tokens.push(unit.clone());
            at += 1;
            continue;
        }
        let start = at;
        while at < units.len()
            && units[at].text.chars().all(is_han)
            && (at == start || units[at - 1].offset_to == units[at].offset_from)
        {
            at += 1;
        }
        let offset = units[start].offset_from;
        let end = units[at - 1].offset_to;
        for word in JIEBA.cut(&text[offset..end], true) {
            tokens.push(Token {
                offset_from: offset + word.byte_start,
                offset_to: offset + word.byte_end,
                position: units[start].position + word.start,
                text: word.word.to_owned(),
                position_length: word.end - word.start,
            });
        }
    }
    tokens
}

/// Encoded Han positions are separate from word terms. A compound can match
/// its constituent word without letting a quoted phrase skip characters.
pub(crate) fn position_term(token: &Token) -> String {
    if token.text.chars().all(is_han) {
        format!("\u{1f}han:{}", token.text)
    } else {
        token.text.clone()
    }
}
fn index_tokens(text: &str) -> Vec<Token> {
    let mut tokens = analyze(text);
    tokens.extend(
        position_tokens(text)
            .into_iter()
            .filter(|token| token.text.chars().all(is_han))
            .map(|mut token| {
                token.text = position_term(&token);
                token
            }),
    );
    tokens.sort_by_key(|token| (token.position, token.offset_to));
    tokens
}

/// A punctuation-bearing scientific symbol is an adjacent lexical unit.
/// This prevents IL-6 matching IL-10 in a document that also mentions chr6.
/// It does not add stemming, typo correction, or synonym expansion.
pub(crate) fn adjacent_symbol(value: &str) -> bool {
    !value.chars().any(char::is_whitespace)
        && value
            .chars()
            .any(|ch| matches!(ch, '-' | '‐' | '‑' | '–' | '*' | ':'))
        && value.chars().any(char::is_alphabetic)
        && (value.chars().any(|ch| ch.is_ascii_digit())
            || value.chars().any(|ch| matches!(ch as u32, 0x0370..=0x03ff)))
}

#[derive(Clone, Default)]
pub(crate) struct BibliographyTokenizer;
pub(crate) struct BibliographyTokenStream {
    tokens: std::vec::IntoIter<Token>,
    current: Token,
}
impl Tokenizer for BibliographyTokenizer {
    type TokenStream<'a> = BibliographyTokenStream;
    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        BibliographyTokenStream {
            tokens: index_tokens(text).into_iter(),
            current: Token::default(),
        }
    }
}
impl TokenStream for BibliographyTokenStream {
    fn advance(&mut self) -> bool {
        if let Some(token) = self.tokens.next() {
            self.current = token;
            true
        } else {
            false
        }
    }
    fn token(&self) -> &Token {
        &self.current
    }
    fn token_mut(&mut self) -> &mut Token {
        &mut self.current
    }
}

pub(crate) fn highlight_ranges(query: &str, snippet: &str) -> Vec<bib_types::SearchHighlight> {
    let query = position_tokens(query);
    if query.is_empty() {
        return Vec::new();
    }
    let tokens = position_tokens(snippet);
    let mut bytes = Vec::new();
    for window in tokens.windows(query.len()) {
        if window.iter().zip(&query).all(|(token, query_token)| {
            token.text == query_token.text
                && token.position - window[0].position == query_token.position - query[0].position
        }) {
            bytes.push((window[0].offset_from, window[window.len() - 1].offset_to));
        }
    }
    if bytes.is_empty() {
        let wanted: std::collections::HashSet<_> = query.into_iter().map(|t| t.text).collect();
        bytes.extend(
            tokens
                .into_iter()
                .filter(|t| wanted.contains(&t.text))
                .map(|t| (t.offset_from, t.offset_to)),
        );
    }
    let mut ranges: Vec<bib_types::SearchHighlight> = Vec::new();
    for (from, to) in bytes {
        let start = snippet[..from].chars().count();
        let end = start + snippet[from..to].chars().count();
        if let Some(previous) = ranges.last_mut() {
            if previous.end >= start {
                previous.end = previous.end.max(end);
                continue;
            }
        }
        ranges.push(bib_types::SearchHighlight { start, end });
    }
    ranges
}
