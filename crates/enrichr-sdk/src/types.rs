//! Typed models for Enrichr and Speedrichr responses.
//!
//! Enrichr serializes enrichment rows as positional JSON arrays
//! (`[rank, term, p_value, z_score, combined_score, overlapping_genes,
//! adjusted_p_value, old_p_value, old_adjusted_p_value]`) rather than
//! objects, so [`GeneSetTerm`] implements [`serde::Deserialize`] by hand.
//! Numbers are coerced leniently because Speedrichr emits bare `Infinity`
//! literals (sanitized to `±1e308` by the client before parsing) and some
//! fields arrive as numeric strings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::EnrichrError;

/// Acknowledgement of a gene list submitted with `addList`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedList {
    #[serde(rename = "userListId")]
    pub user_list_id: u64,
    #[serde(rename = "shortId", default)]
    pub short_id: Option<String>,
}

impl AddedList {
    /// True when the list carries a shareable short ID.
    pub fn has_share_id(&self) -> bool {
        self.short_id.as_deref().is_some_and(|id| !id.is_empty())
    }
}

/// A previously submitted gene list, returned by `view`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewedList {
    #[serde(default)]
    pub genes: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// One library advertised by `datasetStatistics`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryStats {
    #[serde(rename = "libraryName")]
    pub library_name: String,
    #[serde(rename = "numTerms", default)]
    pub num_terms: Option<u64>,
    #[serde(rename = "geneCoverage", default)]
    pub gene_coverage: Option<u64>,
    #[serde(rename = "genesPerTerm", default)]
    pub genes_per_term: Option<f64>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(rename = "categoryId", default)]
    pub category_id: Option<u64>,
}

/// Response of `datasetStatistics`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetStatistics {
    #[serde(default)]
    pub statistics: Vec<LibraryStats>,
}

impl DatasetStatistics {
    /// Sorted library names, the values accepted as `backgroundType`.
    pub fn library_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .statistics
            .iter()
            .map(|row| row.library_name.clone())
            .collect();
        names.sort_unstable();
        names
    }

    /// Libraries whose name contains `needle` (case-insensitive).
    pub fn filter(&self, needle: &str) -> Vec<&LibraryStats> {
        let needle = needle.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return self.statistics.iter().collect();
        }
        self.statistics
            .iter()
            .filter(|row| row.library_name.to_ascii_lowercase().contains(&needle))
            .collect()
    }
}

/// Background token returned by Speedrichr `addbackground`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeedrichrBackground {
    #[serde(rename = "backgroundid")]
    pub background_id: String,
}

/// One enriched gene-set term.
///
/// Field `z_score` holds the fourth element of the positional row: Enrichr's
/// API documentation labels it the z-score, the `export` TSV labels the same
/// quantity "Odds Ratio", and Speedrichr computes the odds ratio — the slot
/// is endpoint-dependent, the position is not.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GeneSetTerm {
    pub rank: u64,
    pub term: String,
    pub p_value: f64,
    pub z_score: f64,
    pub combined_score: f64,
    pub overlapping_genes: Vec<String>,
    pub adjusted_p_value: f64,
    pub old_p_value: f64,
    pub old_adjusted_p_value: f64,
}

impl GeneSetTerm {
    /// Number of query genes overlapping this term.
    pub fn overlap_count(&self) -> usize {
        self.overlapping_genes.len()
    }
}

impl<'de> Deserialize<'de> for GeneSetTerm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let row = Vec::<serde_json::Value>::deserialize(deserializer)?;
        deserialize_term(&row).map_err(serde::de::Error::custom)
    }
}

/// Parse one positional enrichment row. Rows longer than nine elements are
/// accepted and the trailing slots ignored for forward compatibility.
fn deserialize_term(row: &[serde_json::Value]) -> Result<GeneSetTerm, String> {
    const MIN_COLUMNS: usize = 9;
    let column = |index: usize| -> Result<&serde_json::Value, String> {
        row.get(index).ok_or_else(|| {
            format!(
                "enrichment row has {} columns, expected at least {MIN_COLUMNS}",
                row.len()
            )
        })
    };
    Ok(GeneSetTerm {
        rank: value_u64(column(0)?, "rank")?,
        term: value_string(column(1)?, "term")?,
        p_value: value_f64(column(2)?, "p_value")?,
        z_score: value_f64(column(3)?, "z_score")?,
        combined_score: value_f64(column(4)?, "combined_score")?,
        overlapping_genes: value_genes(column(5)?)?,
        adjusted_p_value: value_f64(column(6)?, "adjusted_p_value")?,
        old_p_value: value_f64(column(7)?, "old_p_value")?,
        old_adjusted_p_value: value_f64(column(8)?, "old_adjusted_p_value")?,
    })
}

/// Enrichment results for one library: the single-key object returned by
/// `enrich` and `backgroundenrich`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnrichmentResult {
    pub library: String,
    pub terms: Vec<GeneSetTerm>,
}

impl EnrichmentResult {
    /// Build from the `{library: [rows...]}` response object.
    pub fn from_singleton_map(
        map: BTreeMap<String, Vec<GeneSetTerm>>,
    ) -> Result<Self, EnrichrError> {
        let Some((library, terms)) = map.into_iter().next() else {
            return Err(EnrichrError::Api(
                "Enrichr returned an empty result; the backgroundType is probably not a known \
                 library — call `libraries` to list valid names"
                    .into(),
            ));
        };
        Ok(Self { library, terms })
    }

    /// Terms whose adjusted p-value is at or below `threshold`.
    pub fn significant(&self, threshold: f64) -> impl Iterator<Item = &GeneSetTerm> {
        self.terms
            .iter()
            .filter(move |term| term.adjusted_p_value <= threshold)
    }
}

/// Terms containing one gene, grouped by library — the `genemap` response.
///
/// The endpoint also returns a `descriptions` catalog of every library; it
/// duplicates [`DatasetStatistics`] and is not modeled.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GeneMap {
    /// The queried gene symbol.
    pub gene: String,
    /// Library name to the terms in that library containing the gene.
    pub libraries: BTreeMap<String, Vec<String>>,
}

impl GeneMap {
    /// Build from the `{"gene": {library: [terms...]}}` response object.
    pub fn from_envelope(gene: &str, libraries: BTreeMap<String, Vec<String>>) -> Self {
        Self {
            gene: gene.to_owned(),
            libraries,
        }
    }

    /// Total number of terms across every library.
    pub fn term_count(&self) -> usize {
        self.libraries.values().map(Vec::len).sum()
    }
}

// ---------------------------------------------------------------------------
// Lenient value coercion
// ---------------------------------------------------------------------------

fn value_u64(value: &serde_json::Value, field: &'static str) -> Result<u64, String> {
    match value {
        serde_json::Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| format!("{field} is not an unsigned integer")),
        serde_json::Value::String(text) => text
            .trim()
            .parse()
            .map_err(|_| format!("{field} is neither a number nor a numeric string: {text:?}")),
        other => Err(format!(
            "{field} has unexpected JSON type: {other}",
            other = other
        )),
    }
}

fn value_f64(value: &serde_json::Value, field: &'static str) -> Result<f64, String> {
    match value {
        serde_json::Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| format!("{field} is not representable as f64")),
        serde_json::Value::String(text) => text
            .trim()
            .parse()
            .map_err(|_| format!("{field} is neither a number nor a numeric string: {text:?}")),
        other => Err(format!("{field} has unexpected JSON type: {other}")),
    }
}

fn value_string(value: &serde_json::Value, field: &'static str) -> Result<String, String> {
    match value {
        serde_json::Value::String(text) => Ok(text.clone()),
        other => Err(format!("{field} has unexpected JSON type: {other}")),
    }
}

/// Overlapping genes arrive as a JSON array of symbols; tolerate a joined
/// string (`A;B` or `A,B`) for older Speedrichr deployments.
fn value_genes(value: &serde_json::Value) -> Result<Vec<String>, String> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("overlapping_genes contains a non-string entry: {item}"))
            })
            .collect(),
        serde_json::Value::String(joined) => Ok(joined
            .split([';', ','])
            .map(str::trim)
            .filter(|gene| !gene.is_empty())
            .map(str::to_owned)
            .collect()),
        serde_json::Value::Null => Ok(Vec::new()),
        other => Err(format!(
            "overlapping_genes has unexpected JSON type: {other}"
        )),
    }
}
