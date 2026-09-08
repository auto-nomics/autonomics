//! Builder for UniProtKB query expressions.
//!
//! UniProt search queries are field-scoped boolean expressions such as
//! `gene:INS AND organism_id:9606 AND reviewed:true`. This module provides
//! [`Query`], a chainable builder that renders correct syntax for the
//! common fields and keeps raw clauses available as an escape hatch.
//!
//! ## Common search fields
//!
//! | Field           | Example                     |
//! |-----------------|-----------------------------|
//! | Accession       | `accession:(P01308 OR P01309)` |
//! | Entry name      | `uni_prot_id:INS_HUMAN`     |
//! | Gene name       | `gene:INS`                  |
//! | Protein name    | `protein_name:insulin`      |
//! | Organism name   | `organism_name:"Homo sapiens"` |
//! | Organism ID     | `organism_id:9606`          |
//! | Keyword         | `keyword:Glycoprotein`      |
//! | Reviewed only   | `reviewed:true`             |
//!
//! Boolean operators (uppercase): `AND`, `OR`, `NOT`. Phrases containing
//! whitespace are double-quoted.
//!
//! ## Example
//!
//! ```
//! use uniprot::query::Query;
//!
//! let q = Query::new()
//!     .gene(["INS"])
//!     .organism_id(9606)
//!     .reviewed(true)
//!     .build()
//!     .unwrap();
//! assert_eq!(q, "gene:INS AND organism_id:9606 AND reviewed:true");
//! ```

use crate::error::{Result, UniProtError};

/// A builder for UniProtKB query expressions.
///
/// Every method adds one clause; clauses are joined with `AND`. Calling a
/// method with multiple values produces a single OR-grouped clause.
#[derive(Debug, Clone, Default)]
pub struct Query {
    clauses: Vec<String>,
}

impl Query {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a raw, pre-rendered clause — the escape hatch for fields the
    /// builder does not model, e.g. `Query::new().raw("length:[500 TO 1000]")`.
    pub fn raw(mut self, clause: impl Into<String>) -> Self {
        let clause = clause.into();
        if !clause.trim().is_empty() {
            self.clauses.push(clause.trim().to_owned());
        }
        self
    }

    /// Restrict by gene name(s), e.g. `gene:(BRCA1 OR BRCA2)`.
    pub fn gene<I, S>(mut self, genes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.push_field("gene", genes);
        self
    }

    /// Restrict by protein name, e.g. `protein_name:insulin`.
    pub fn protein_name<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.push_field("protein_name", names);
        self
    }

    /// Restrict by organism name, quoted when needed, e.g.
    /// `organism_name:"Homo sapiens"`.
    pub fn organism<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.push_field("organism_name", names);
        self
    }

    /// Restrict by NCBI taxon ID, e.g. `organism_id:9606`.
    pub fn organism_id(mut self, taxon_id: u64) -> Self {
        self.clauses.push(format!("organism_id:{taxon_id}"));
        self
    }

    /// Restrict by accession(s), e.g. `accession:(P01308 OR P0DTC2)`.
    pub fn accessions<I, S>(mut self, accessions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.push_field("accession", accessions);
        self
    }

    /// Restrict by keyword, e.g. `keyword:Glycoprotein`.
    pub fn keyword<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.push_field("keyword", keywords);
        self
    }

    /// Restrict to reviewed (Swiss-Prot) or unreviewed (TrEMBL) entries.
    pub fn reviewed(mut self, reviewed: bool) -> Self {
        self.clauses.push(format!("reviewed:{reviewed}"));
        self
    }

    /// Render the query expression. Errors if no clause was added.
    pub fn build(self) -> Result<String> {
        if self.clauses.is_empty() {
            return Err(UniProtError::Param(
                "query is empty: add at least one clause".into(),
            ));
        }
        Ok(self.clauses.join(" AND "))
    }

    /// Render `field:value` (single value) or `field:(v1 OR v2)` (multiple),
    /// skipping empty values, and push the clause when non-empty.
    fn push_field<I, S>(&mut self, field: &str, values: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let values: Vec<String> = values
            .into_iter()
            .map(|v| quote_term(v.as_ref()))
            .filter(|v| !v.is_empty())
            .collect();
        match values.len() {
            0 => {}
            1 => self.clauses.push(format!("{field}:{}", values[0])),
            _ => self
                .clauses
                .push(format!("{field}:({})", values.join(" OR "))),
        }
    }
}

/// Wrap a term in double quotes iff it contains whitespace or characters
/// that would break the query grammar. Internal `"` characters are
/// backslash-escaped.
fn quote_term(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let needs_quotes = trimmed
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == ':' || c == '(' || c == ')');
    if needs_quotes {
        let escaped = trimmed.replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        trimmed.to_string()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn one(v: &str) -> Vec<&str> {
        vec![v]
    }

    #[test]
    fn single_field_single_value() {
        let q = Query::new().gene(one("INS")).build().unwrap();
        assert_eq!(q, "gene:INS");
    }

    #[test]
    fn multiple_values_group_with_or() {
        let q = Query::new()
            .accessions(["P01308", "P0DTC2"])
            .build()
            .unwrap();
        assert_eq!(q, "accession:(P01308 OR P0DTC2)");
    }

    #[test]
    fn clauses_join_with_and() {
        let q = Query::new()
            .gene(one("INS"))
            .organism_id(9606)
            .reviewed(true)
            .build()
            .unwrap();
        assert_eq!(q, "gene:INS AND organism_id:9606 AND reviewed:true");
    }

    #[test]
    fn reviewed_false_renders() {
        let q = Query::new().reviewed(false).build().unwrap();
        assert_eq!(q, "reviewed:false");
    }

    #[test]
    fn whitespace_terms_are_quoted() {
        let q = Query::new().organism(one("Homo sapiens")).build().unwrap();
        assert_eq!(q, r#"organism_name:"Homo sapiens""#);
    }

    #[test]
    fn empty_values_are_dropped() {
        let q = Query::new().gene(["", "  ", "INS"]).build().unwrap();
        assert_eq!(q, "gene:INS");
    }

    #[test]
    fn all_values_empty_adds_no_clause() {
        let q = Query::new().gene(["", "  "]).build();
        assert!(q.is_err());
    }

    #[test]
    fn empty_query_errors() {
        assert!(Query::new().build().is_err());
    }

    #[test]
    fn raw_clause_passthrough() {
        let q = Query::new()
            .organism_id(9606)
            .raw("length:[500 TO 1000]")
            .build()
            .unwrap();
        assert_eq!(q, "organism_id:9606 AND length:[500 TO 1000]");
    }

    #[test]
    fn blank_raw_clause_is_ignored() {
        let q = Query::new().raw("   ").gene(one("INS")).build().unwrap();
        assert_eq!(q, "gene:INS");
    }

    #[test]
    fn special_characters_trigger_quoting() {
        assert_eq!(quote_term("plain"), "plain");
        assert_eq!(quote_term("  trim  "), "trim");
        assert_eq!(quote_term("spike protein"), "\"spike protein\"");
        assert_eq!(quote_term("a:b"), "\"a:b\"");
        assert_eq!(quote_term("(x)"), "\"(x)\"");
        assert_eq!(quote_term("say \"hi\""), "\"say \\\"hi\\\"\"");
    }
}
