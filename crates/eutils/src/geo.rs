//! GEO-specific helpers built on the NCBI E-utilities `gds` database.
//!
//! GEO DataSets is exposed through Entrez as database `gds`. The generic
//! E-utilities methods already accept that database name; this module adds the
//! GEO query fields, validates accession identifiers, and parses GEO DataSets
//! summaries into typed records.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::EutilsClient;
use crate::error::{EutilsError, Result};
use crate::types::{EFetchRequest, ESearchRequest, ESearchResponse, ESummaryRequest};

/// Entrez database for GEO DataSets records (GSE, GDS, GSM, and GPL).
pub const GEO_DATASETS_DB: &str = "gds";

/// Maximum number of UIDs sent to ESummary in one request.
const GEO_SUMMARY_BATCH_SIZE: usize = 200;

/// GEO record type represented by an accession prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum GeoEntryType {
    Series,
    DataSet,
    Sample,
    Platform,
}

impl GeoEntryType {
    fn tag(self) -> &'static str {
        match self {
            Self::Series => "GSE",
            Self::DataSet => "GDS",
            Self::Sample => "GSM",
            Self::Platform => "GPL",
        }
    }

    fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "GSE" => Some(Self::Series),
            "GDS" => Some(Self::DataSet),
            "GSM" => Some(Self::Sample),
            "GPL" => Some(Self::Platform),
            _ => None,
        }
    }
}

/// Search parameters for GEO DataSets.
///
/// All populated filters are joined with Entrez `AND`. A free-text `term` is
/// searched across all GEO DataSets fields.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GeoSearchRequest {
    /// Free-text query across GEO DataSets fields.
    pub term: Option<String>,
    /// Exact GEO accession, such as `GSE48094`.
    pub accession: Option<String>,
    /// Organism filter, such as `Homo sapiens`.
    pub organism: Option<String>,
    /// GEO record type filter.
    pub entry_type: Option<GeoEntryType>,
    /// GEO dataset type filter, such as `Expression profiling by high throughput sequencing`.
    pub dataset_type: Option<String>,
    /// Maximum number of UIDs to return.
    pub retmax: Option<u32>,
    /// Sequential start index for pagination.
    pub retstart: Option<u32>,
    /// Entrez sort expression when supported by the database.
    pub sort: Option<String>,
    /// Save the search result in the Entrez History server.
    pub usehistory: Option<bool>,
}

impl GeoSearchRequest {
    /// Create a free-text GEO search.
    pub fn new(term: impl Into<String>) -> Self {
        Self {
            term: Some(term.into()),
            ..Self::default()
        }
    }

    /// Create an exact GEO accession search.
    ///
    /// The accession prefix also selects the entry type, which prevents the
    /// linked platform and sample records from appearing in the result set.
    pub fn accession(accession: impl AsRef<str>) -> Result<Self> {
        let accession = normalize_accession(accession.as_ref())?;
        let entry_type = GeoEntryType::from_prefix(&accession[..3])
            .ok_or_else(|| EutilsError::Param(format!("unsupported GEO accession: {accession}")))?;
        Ok(Self {
            accession: Some(accession),
            entry_type: Some(entry_type),
            ..Self::default()
        })
    }

    /// Add an organism filter.
    pub fn with_organism(mut self, organism: impl Into<String>) -> Self {
        self.organism = Some(organism.into());
        self
    }

    /// Add a GEO dataset type filter.
    pub fn with_dataset_type(mut self, dataset_type: impl Into<String>) -> Self {
        self.dataset_type = Some(dataset_type.into());
        self
    }

    /// Set the page size.
    pub fn with_retmax(mut self, retmax: u32) -> Self {
        self.retmax = Some(retmax);
        self
    }

    /// Set the page start index.
    pub fn with_retstart(mut self, retstart: u32) -> Self {
        self.retstart = Some(retstart);
        self
    }

    /// Restrict results to a GEO entry type.
    pub fn with_entry_type(mut self, entry_type: GeoEntryType) -> Self {
        self.entry_type = Some(entry_type);
        self
    }

    /// Set the Entrez sort expression.
    pub fn with_sort(mut self, sort: impl Into<String>) -> Self {
        self.sort = Some(sort.into());
        self
    }

    /// Save the result in the Entrez History server.
    pub fn with_history(mut self) -> Self {
        self.usehistory = Some(true);
        self
    }

    pub(crate) fn into_esearch_request(self) -> Result<ESearchRequest> {
        let accession = match &self.accession {
            Some(accession) => Some(normalize_accession(accession)?),
            None => None,
        };
        let query = build_query(
            &self.term,
            accession.as_deref(),
            &self.organism,
            self.entry_type,
            &self.dataset_type,
        )?;
        let mut request = ESearchRequest::new(GEO_DATASETS_DB, &query);
        request.retmax = self.retmax;
        request.retstart = self.retstart;
        request.sort = self.sort;
        request.usehistory = self.usehistory;
        Ok(request)
    }
}

fn build_query(
    term: &Option<String>,
    accession: Option<&str>,
    organism: &Option<String>,
    entry_type: Option<GeoEntryType>,
    dataset_type: &Option<String>,
) -> Result<String> {
    let mut clauses = Vec::new();

    if let Some(term) = nonempty(term) {
        clauses.push(quote_entrez_term(term));
    }
    if let Some(accession) = accession {
        clauses.push(format!("{accession}[ACCN]"));
    }
    if let Some(organism) = nonempty(organism) {
        clauses.push(format!("{}[ORGN]", quote_entrez_term(organism)));
    }
    if let Some(entry_type) = entry_type {
        clauses.push(format!("{}[ETYP]", entry_type.tag()));
    }
    if let Some(dataset_type) = nonempty(dataset_type) {
        clauses.push(format!("{}[GTYP]", quote_entrez_term(dataset_type)));
    }

    if clauses.is_empty() {
        return Err(EutilsError::Param(
            "GEO search is empty: provide a term, accession, or filter".into(),
        ));
    }
    Ok(clauses.join(" AND "))
}

fn nonempty(value: &Option<String>) -> Option<&str> {
    let value = value.as_deref()?.trim();
    (!value.is_empty()).then_some(value)
}

fn quote_entrez_term(term: &str) -> String {
    let trimmed = term.trim();
    if trimmed
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '"'))
    {
        format!("\"{}\"", trimmed.replace('"', "\\\""))
    } else {
        trimmed.to_owned()
    }
}

fn normalize_accession(input: &str) -> Result<String> {
    let accession = input.trim().to_ascii_uppercase();
    let (prefix, number) = accession.split_once('-').unwrap_or((
        accession.get(..3).unwrap_or_default(),
        accession.get(3..).unwrap_or_default(),
    ));
    if GeoEntryType::from_prefix(prefix).is_none()
        || number.is_empty()
        || !number.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(EutilsError::Param(format!(
            "invalid GEO accession '{input}': expected GSE, GDS, GSM, or GPL followed by digits"
        )));
    }
    Ok(format!("{prefix}{number}"))
}

/// A sample listed in a GEO Series summary.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GeoSample {
    #[serde(rename = "accession")]
    pub accession: String,
    #[serde(rename = "title")]
    pub title: String,
}

/// Typed GEO DataSets document summary.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GeoSummary {
    pub uid: String,
    pub accession: String,
    pub title: String,
    pub summary: String,
    pub taxon: String,
    #[serde(rename = "entrytype")]
    pub entry_type: String,
    #[serde(rename = "gdstype")]
    pub dataset_type: String,
    #[serde(rename = "ptechtype")]
    pub platform_technology_type: String,
    #[serde(rename = "valtype")]
    pub value_type: String,
    pub gpl: String,
    pub gse: String,
    #[serde(rename = "pdat")]
    pub publication_date: String,
    #[serde(rename = "suppfile")]
    pub supplementary_files: String,
    pub samples: Vec<GeoSample>,
    #[serde(default, deserialize_with = "deserialize_optional_count")]
    pub n_samples: Option<u64>,
    pub pubmedids: Vec<String>,
    pub projects: Vec<String>,
    #[serde(rename = "ftplink")]
    pub ftp_link: String,
    #[serde(rename = "geo2r")]
    pub geo2r: String,
    pub bioproject: String,
    #[serde(default)]
    pub relations: Vec<Value>,
    #[serde(default)]
    pub extrelations: Vec<Value>,
}

fn deserialize_optional_count<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Null => Ok(None),
        Value::Number(number) => number
            .as_u64()
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("GEO n_samples is not an unsigned integer")),
        Value::String(text) if text.trim().is_empty() => Ok(None),
        Value::String(text) => text.trim().parse::<u64>().map(Some).map_err(|_| {
            serde::de::Error::custom(format!("invalid GEO n_samples value: '{text}'"))
        }),
        other => Err(serde::de::Error::custom(format!(
            "invalid GEO n_samples value: {other}"
        ))),
    }
}

/// ESearch result plus typed summaries for the returned UID page.
#[derive(Debug, Clone)]
pub struct GeoSearchResults {
    pub search: ESearchResponse,
    pub records: Vec<GeoSummary>,
}

#[derive(Debug, Deserialize)]
struct GeoSummaryEnvelope {
    result: GeoSummaryResult,
}

#[derive(Debug, Deserialize)]
struct GeoSummaryResult {
    uids: Vec<String>,
    #[serde(flatten)]
    records: HashMap<String, GeoSummary>,
}

impl EutilsClient {
    /// Search GEO DataSets and return matching Entrez UIDs.
    pub async fn geo_search(&self, request: &GeoSearchRequest) -> Result<ESearchResponse> {
        self.esearch(&request.clone().into_esearch_request()?).await
    }

    /// Search GEO DataSets and fetch typed summaries for the returned UIDs.
    ///
    /// This makes one ESearch request and one ESummary request when results
    /// are present.
    pub async fn geo_search_records(&self, request: &GeoSearchRequest) -> Result<GeoSearchResults> {
        let search = self.geo_search(request).await?;
        let records = if search.result.id_list.is_empty() {
            Vec::new()
        } else {
            self.geo_summaries(search.result.id_list.clone()).await?
        };
        Ok(GeoSearchResults { search, records })
    }

    /// Resolve an exact GEO accession to its typed GEO DataSets summaries.
    ///
    /// This normally returns one record. An empty vector means the accession
    /// was not found.
    pub async fn geo_lookup_accession(
        &self,
        accession: impl AsRef<str>,
    ) -> Result<Vec<GeoSummary>> {
        let request = GeoSearchRequest::accession(accession)?;
        let results = self.geo_search_records(&request).await?;
        let accession = request.accession.as_deref().unwrap_or_default();
        Ok(results
            .records
            .into_iter()
            .filter(|record| record.accession == accession)
            .collect())
    }

    /// Fetch typed GEO DataSets summaries for Entrez UIDs.
    ///
    /// UIDs are automatically batched into groups of 200.
    pub async fn geo_summaries<I, T>(&self, uids: I) -> Result<Vec<GeoSummary>>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<str>,
    {
        let owned: Vec<String> = uids
            .into_iter()
            .map(|uid| uid.as_ref().trim().to_owned())
            .filter(|uid| !uid.is_empty())
            .collect();
        if owned.is_empty() {
            return Ok(Vec::new());
        }
        if owned
            .iter()
            .any(|uid| !uid.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(EutilsError::Param(
                "GEO UIDs must contain only digits".into(),
            ));
        }

        let mut records = Vec::with_capacity(owned.len());
        for batch in owned.chunks(GEO_SUMMARY_BATCH_SIZE) {
            let id = batch.join(",");
            let value = self
                .esummary(&ESummaryRequest::new(GEO_DATASETS_DB, &id))
                .await?;
            let envelope: GeoSummaryEnvelope = serde_json::from_value(value)?;
            let result = envelope.result;
            records.extend(
                result
                    .uids
                    .into_iter()
                    .filter_map(|uid| result.records.get(&uid).cloned()),
            );
        }
        Ok(records)
    }

    /// Fetch a GEO DataSets record as text using an Entrez UID.
    pub async fn geo_fetch(
        &self,
        uid: impl AsRef<str>,
        rettype: Option<&str>,
        retmode: Option<&str>,
    ) -> Result<String> {
        let uid = uid.as_ref().trim();
        if uid.is_empty() || !uid.bytes().all(|b| b.is_ascii_digit()) {
            return Err(EutilsError::Param(
                "GEO UIDs must contain only digits".into(),
            ));
        }
        self.efetch(&EFetchRequest {
            db: GEO_DATASETS_DB.to_owned(),
            id: uid.to_owned(),
            rettype: rettype.map(str::to_owned),
            retmode: retmode.map(str::to_owned),
            ..Default::default()
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_free_text_and_filter_query() {
        let request = GeoSearchRequest::new("single cell RNA-seq")
            .with_organism("Homo sapiens")
            .with_dataset_type("Expression profiling by high throughput sequencing")
            .with_retmax(10);
        let entrez = request.into_esearch_request().unwrap();
        assert_eq!(entrez.db, "gds");
        assert_eq!(
            entrez.term,
            concat!(
                "\"single cell RNA-seq\" AND \"Homo sapiens\"[ORGN] AND ",
                "\"Expression profiling by high throughput sequencing\"[GTYP]"
            )
        );
        assert_eq!(entrez.retmax, Some(10));
    }

    #[test]
    fn builds_exact_series_accession_query() {
        let request = GeoSearchRequest::accession("gse-480_94").map(|r| r.with_retstart(20));
        assert!(request.is_err());
        let request = GeoSearchRequest::accession("gse48094").unwrap();
        let entrez = request.into_esearch_request().unwrap();
        assert_eq!(entrez.term, "GSE48094[ACCN] AND GSE[ETYP]");
    }

    #[test]
    fn validates_accessions() {
        assert_eq!(normalize_accession(" gse48094 ").unwrap(), "GSE48094");
        assert_eq!(normalize_accession("GSM-1168417").unwrap(), "GSM1168417");
        assert!(normalize_accession("ABC123").is_err());
        assert!(normalize_accession("GSE").is_err());
        assert!(normalize_accession("GSE48x").is_err());
    }

    #[test]
    fn rejects_empty_search() {
        let error = GeoSearchRequest::default()
            .into_esearch_request()
            .unwrap_err();
        assert!(matches!(error, EutilsError::Param(_)));
    }

    #[test]
    fn parses_geo_summary_count_and_samples() {
        let value = serde_json::json!({
            "uid": "200048094",
            "accession": "GSE48094",
            "title": "Example series",
            "summary": "Example summary",
            "taxon": "Mus musculus",
            "entrytype": "GSE",
            "gdstype": "Non-coding RNA profiling by array",
            "ptechtype": "",
            "valtype": "",
            "gpl": "17315",
            "gse": "48094",
            "pdat": "2013/12/27",
            "suppfile": "TXT, XLS",
            "samples": [
                {"accession": "GSM1168414", "title": "young mice Y1"}
            ],
            "n_samples": 6,
            "pubmedids": ["23946636"],
            "projects": [],
            "ftplink": "ftp://ftp.ncbi.nlm.nih.gov/geo/series/GSE48nnn/GSE48094/",
            "geo2r": "yes",
            "bioproject": "PRJNA208991",
            "relations": [],
            "extrelations": []
        });
        let record: GeoSummary = serde_json::from_value(value).unwrap();
        assert_eq!(record.entry_type, "GSE");
        assert_eq!(record.n_samples, Some(6));
        assert_eq!(record.samples[0].accession, "GSM1168414");
    }

    #[test]
    fn parses_empty_sample_count() {
        let value = serde_json::json!({
            "uid": "301168417",
            "accession": "GSM1168417",
            "title": "Example sample",
            "summary": "Example",
            "taxon": "Mus musculus",
            "entrytype": "GSM",
            "gdstype": "",
            "ptechtype": "",
            "valtype": "",
            "gpl": "17315",
            "gse": "48094",
            "pdat": "2013/12/27",
            "suppfile": "TXT",
            "samples": [],
            "n_samples": "",
            "pubmedids": [],
            "projects": [],
            "ftplink": "ftp://ftp.ncbi.nlm.nih.gov/geo/samples/GSM1168nnn/GSM1168417/",
            "geo2r": "",
            "bioproject": "",
            "relations": [],
            "extrelations": []
        });
        let record: GeoSummary = serde_json::from_value(value).unwrap();
        assert_eq!(record.n_samples, None);
    }
}
