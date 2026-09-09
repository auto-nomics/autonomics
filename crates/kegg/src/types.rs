//! Typed representations of KEGG REST text responses.

use serde::{Deserialize, Serialize};

/// An identifier and description returned by `list` or `find`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntrySummary {
    pub id: String,
    pub description: String,
}

/// Database metadata returned by `info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseInfo {
    pub database: String,
    pub entry_count: Option<u64>,
    pub release_date: Option<String>,
}

/// Parsed database or organism metadata returned by `info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub title: String,
    pub summary: String,
    pub databases: Vec<DatabaseInfo>,
    pub linked_databases: Vec<String>,
    pub raw: String,
}

/// One source/target relationship returned by `conv` or `link`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    pub source: String,
    pub target: String,
}

/// One drug-drug interaction returned by `ddi`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrugInteraction {
    pub drug: String,
    pub interacts_with: String,
    pub category: String,
    pub description: String,
}

/// Header metadata parsed from a KEGG flat-file entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlatEntry {
    pub id: String,
    pub entry_type: Option<String>,
    pub organism: Option<String>,
    pub raw: String,
}

/// Binary content such as a pathway image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binary {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl Binary {
    /// Decode the content as UTF-8 text when callers know it is textual.
    pub fn text(&self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.bytes.clone())
    }
}
