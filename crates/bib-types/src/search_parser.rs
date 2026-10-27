//! A small, bounded query language shared by local and external adapters.
//! Boolean precedence is NOT, AND (including whitespace), OR. Quoted text
//! means an adjacent phrase; field prefixes may also apply to groups.

use crate::{SearchExpr, SearchField};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchParseError {
    pub message: String,
    pub position: usize,
}
impl std::fmt::Display for SearchParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.message, self.position)
    }
}
impl std::error::Error for SearchParseError {}

#[derive(Debug, Clone)]
enum Kind {
    Word(String),
    Phrase(String),
    Open,
    Close,
    And,
    Or,
    Not,
}
#[derive(Debug, Clone)]
struct Token {
    kind: Kind,
    position: usize,
}

pub fn parse_search_query(input: &str) -> Result<Option<SearchExpr>, SearchParseError> {
    if input.len() > 16_384 {
        return Err(SearchParseError {
            message: "query exceeds 16384 bytes".into(),
            position: 16_384,
        });
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let lower = trimmed.to_lowercase();
    if !trimmed.chars().any(char::is_whitespace)
        && (lower.starts_with("https://doi.org/")
            || lower.starts_with("http://doi.org/")
            || (lower.starts_with("10.") && lower.contains('/')))
    {
        return Ok(Some(SearchExpr::Term {
            value: trimmed.into(),
            field: Some(SearchField::Identifier),
        }));
    }
    let tokens = lex(input)?;
    let mut parser = Parser {
        tokens,
        at: 0,
        end: input.len(),
        depth: 0,
    };
    let expr = parser.or()?;
    if parser.at != parser.tokens.len() {
        return Err(parser.error("unexpected closing parenthesis or operator"));
    }
    Ok(Some(expr))
}

fn lex(input: &str) -> Result<Vec<Token>, SearchParseError> {
    let mut chars = input.char_indices().peekable();
    let mut tokens = Vec::new();
    while let Some((position, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        let kind = match ch {
            '(' => Kind::Open,
            ')' => Kind::Close,
            '"' => {
                let mut value = String::new();
                let mut closed = false;
                while let Some((_, ch)) = chars.next() {
                    match ch {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => {
                            if let Some((_, escaped)) = chars.next() {
                                value.push(escaped);
                            }
                        }
                        other => value.push(other),
                    }
                }
                if !closed || value.trim().is_empty() {
                    return Err(SearchParseError {
                        message: "expected a nonempty, closed quoted phrase".into(),
                        position,
                    });
                }
                Kind::Phrase(value)
            }
            _ => {
                let mut value = ch.to_string();
                while let Some((_, next)) = chars.peek() {
                    if next.is_whitespace() || matches!(next, '(' | ')' | '"') {
                        break;
                    }
                    value.push(chars.next().unwrap().1);
                }
                match value.as_str() {
                    "AND" => Kind::And,
                    "OR" => Kind::Or,
                    "NOT" => Kind::Not,
                    _ => Kind::Word(value),
                }
            }
        };
        tokens.push(Token { kind, position });
        if tokens.len() > 256 {
            return Err(SearchParseError {
                message: "query exceeds 256 tokens".into(),
                position,
            });
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
    end: usize,
    depth: usize,
}
impl Parser {
    fn error(&self, message: &str) -> SearchParseError {
        SearchParseError {
            message: message.into(),
            position: self
                .tokens
                .get(self.at)
                .map(|token| token.position)
                .unwrap_or(self.end),
        }
    }
    fn peek(&self) -> Option<&Kind> {
        self.tokens.get(self.at).map(|token| &token.kind)
    }
    fn or(&mut self) -> Result<SearchExpr, SearchParseError> {
        let mut clauses = vec![self.and()?];
        while matches!(self.peek(), Some(Kind::Or)) {
            self.at += 1;
            clauses.push(self.and()?);
        }
        Ok(if clauses.len() == 1 {
            clauses.remove(0)
        } else {
            SearchExpr::Or { clauses }
        })
    }
    fn and(&mut self) -> Result<SearchExpr, SearchParseError> {
        let mut clauses = vec![self.unary()?];
        loop {
            match self.peek() {
                Some(Kind::And) => {
                    self.at += 1;
                    clauses.push(self.unary()?);
                }
                Some(Kind::Word(_) | Kind::Phrase(_) | Kind::Open | Kind::Not) => {
                    clauses.push(self.unary()?)
                }
                _ => break,
            }
        }
        Ok(if clauses.len() == 1 {
            clauses.remove(0)
        } else {
            SearchExpr::And { clauses }
        })
    }
    fn unary(&mut self) -> Result<SearchExpr, SearchParseError> {
        self.depth += 1;
        if self.depth > 32 {
            return Err(self.error("query exceeds 32 nesting levels"));
        }
        let result = self.unary_inner();
        self.depth -= 1;
        result
    }
    fn unary_inner(&mut self) -> Result<SearchExpr, SearchParseError> {
        let kind = self
            .peek()
            .cloned()
            .ok_or_else(|| self.error("expected a search clause"))?;
        self.at += 1;
        match kind {
            Kind::Not => Ok(SearchExpr::Not {
                clause: Box::new(self.unary()?),
            }),
            Kind::Open => {
                let expr = self.or()?;
                if !matches!(self.peek(), Some(Kind::Close)) {
                    return Err(self.error("expected closing parenthesis"));
                }
                self.at += 1;
                Ok(expr)
            }
            Kind::Phrase(value) => Ok(SearchExpr::Phrase { value, field: None }),
            Kind::Word(value) => {
                if let Some((name, rest)) = value.split_once(':') {
                    if !value.starts_with("http://") && !value.starts_with("https://") {
                        let field = match field(name) {
                            Some(field) => field,
                            None if name.chars().any(|ch| !ch.is_ascii_alphabetic())
                                || name.chars().all(|ch| ch.is_ascii_uppercase()) =>
                            {
                                return Ok(SearchExpr::Term { value, field: None });
                            }
                            None => return Err(self.error("unknown search field")),
                        };
                        if rest.is_empty() {
                            let mut expr = self.unary()?;
                            apply_field(&mut expr, field);
                            return Ok(expr);
                        }
                        if field == SearchField::Year {
                            if let Some((lower, upper)) = rest.split_once("..") {
                                if lower.is_empty() && upper.is_empty() {
                                    return Err(self.error("year range requires a bound"));
                                }
                                return Ok(SearchExpr::Range {
                                    field,
                                    gte: nonempty(lower),
                                    lte: nonempty(upper),
                                });
                            }
                            if let Some(year) = rest.strip_prefix(">=") {
                                return Ok(SearchExpr::Range {
                                    field,
                                    gte: nonempty(year),
                                    lte: None,
                                });
                            }
                            if let Some(year) = rest.strip_prefix("<=") {
                                return Ok(SearchExpr::Range {
                                    field,
                                    gte: None,
                                    lte: nonempty(year),
                                });
                            }
                        }
                        return Ok(SearchExpr::Term {
                            value: rest.into(),
                            field: Some(field),
                        });
                    }
                }
                Ok(SearchExpr::Term { value, field: None })
            }
            Kind::And | Kind::Or | Kind::Close => Err(self.error("expected a search clause")),
        }
    }
}
fn nonempty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.into())
    }
}
fn field(name: &str) -> Option<SearchField> {
    Some(match name.to_ascii_lowercase().as_str() {
        "title" => SearchField::Title,
        "author" | "creator" => SearchField::Creator,
        "abstract" => SearchField::Abstract,
        "journal" | "publication" => SearchField::Publication,
        "identifier" | "doi" | "pmid" | "pmcid" | "arxiv" => SearchField::Identifier,
        "year" => SearchField::Year,
        "keyword" => SearchField::Keyword,
        "tag" => SearchField::Tag,
        "note" => SearchField::Note,
        "fulltext" | "attachment" => SearchField::AttachmentContent,
        "any" => SearchField::Any,
        _ => return None,
    })
}
fn apply_field(expr: &mut SearchExpr, inherited: SearchField) {
    match expr {
        SearchExpr::Term { field, .. } | SearchExpr::Phrase { field, .. } => {
            if field.is_none() {
                *field = Some(inherited);
            }
        }
        SearchExpr::And { clauses } | SearchExpr::Or { clauses } => {
            for clause in clauses {
                apply_field(clause, inherited);
            }
        }
        SearchExpr::Not { clause } => apply_field(clause, inherited),
        SearchExpr::Range { .. } => {}
    }
}
