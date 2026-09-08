//! Typed models for UniProt API responses.
//!
//! The UniProt JSON payloads are far larger than any client needs. These
//! models type the high-value fields and deliberately **ignore unknown
//! fields** (serde's default), so API additions do not break this crate.
//! When full fidelity matters — bulk downloads, archival — use the raw
//! [`Format::Tsv`] / [`Format::Json`] stream accessors on the client instead
//! of the typed ones.

use serde::{Deserialize, Serialize};

// ===========================================================================
// Output formats
// ===========================================================================

/// Response formats offered by the UniProt REST API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    #[default]
    Json,
    Tsv,
    /// Accession-only list, one per line.
    List,
    /// Protein sequences (UniProtKB only).
    Fasta,
    /// GFF3 sequence annotations (UniProtKB only).
    Gff,
    /// Flat-file text (UniProtKB only).
    Txt,
    Xml,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Tsv => "tsv",
            Self::List => "list",
            Self::Fasta => "fasta",
            Self::Gff => "gff",
            Self::Txt => "txt",
            Self::Xml => "xml",
        }
    }

    /// Parse a format name (case-insensitive, leading dot tolerated) —
    /// accepts `json`, `tsv`, `fasta`, … as supplied by users or file
    /// extensions.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('.').to_ascii_lowercase();
        match s.as_str() {
            "json" => Some(Self::Json),
            "tsv" => Some(Self::Tsv),
            "list" => Some(Self::List),
            "fasta" | "fa" | "faa" => Some(Self::Fasta),
            "gff" | "gff3" => Some(Self::Gff),
            "txt" | "text" | "flat" => Some(Self::Txt),
            "xml" => Some(Self::Xml),
            _ => None,
        }
    }
}

// ===========================================================================
// Search requests
// ===========================================================================

/// Parameters for the paginated `/search` endpoints.
///
/// The `query` expression uses UniProt query syntax; build it with the
/// [`crate::query::Query`] builder or pass a raw string for expert mode.
///
/// ```
/// use uniprot::types::SearchRequest;
///
/// let req = SearchRequest::new("accession:P01308 OR accession:P0DTC2")
///     .size(2)
///     .sort("accession asc");
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    /// TSV field names to restrict the output to (ignored for `format=json`
    /// requests, which always return full entries).
    #[serde(default)]
    pub fields: Option<Vec<String>>,
    /// Page size; the API caps this at 500.
    #[serde(default)]
    pub size: Option<u32>,
    /// Opaque cursor from a previous response's `next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Sort expression, e.g. `"accession asc"`.
    #[serde(default)]
    pub sort: Option<String>,
}

impl SearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Default::default()
        }
    }

    pub fn fields<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.fields = Some(fields.into_iter().map(Into::into).collect());
        self
    }

    pub fn size(mut self, size: u32) -> Self {
        self.size = Some(size);
        self
    }

    pub fn cursor(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
        self
    }

    pub fn sort(mut self, sort: impl Into<String>) -> Self {
        self.sort = Some(sort.into());
        self
    }
}

/// One page of search results plus the pagination metadata the API ships in
/// response headers.
#[derive(Debug, Clone)]
pub struct SearchResults<T> {
    pub results: Vec<T>,
    /// `X-Total-Results` header — total hits for the query, not just this
    /// page.
    pub total_results: Option<u64>,
    /// Opaque cursor for the next page (`Link` header, `rel="next"`); absent
    /// on the last page.
    pub next_cursor: Option<String>,
    /// UniProt release name (`X-UniProt-Release` header).
    pub release: Option<String>,
}

impl<T> Default for SearchResults<T> {
    fn default() -> Self {
        Self {
            results: Vec::new(),
            total_results: None,
            next_cursor: None,
            release: None,
        }
    }
}

// The wire format only carries the `results` array; the pagination fields
// come from response headers and are filled in by the client.
impl<'de, T: Deserialize<'de>> Deserialize<'de> for SearchResults<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Envelope<T> {
            results: Vec<T>,
        }
        let env = Envelope::<T>::deserialize(deserializer)?;
        Ok(Self {
            results: env.results,
            total_results: None,
            next_cursor: None,
            release: None,
        })
    }
}

// ===========================================================================
// UniProtKB entry
// ===========================================================================

/// A UniProtKB protein entry (Swiss-Prot reviewed or TrEMBL unreviewed).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    /// e.g. `"UniProtKB reviewed (Swiss-Prot)"`.
    pub entry_type: String,
    pub primary_accession: String,
    /// e.g. `"INS_HUMAN"`.
    pub uni_protkb_id: String,
    pub secondary_accessions: Vec<String>,
    pub entry_audit: EntryAudit,
    /// 1–5 annotation confidence score.
    pub annotation_score: Option<f64>,
    pub organism: Organism,
    pub organism_hosts: Vec<Organism>,
    /// e.g. `"1: Evidence at protein level"`.
    pub protein_existence: Option<String>,
    pub protein_description: ProteinDescription,
    pub genes: Vec<Gene>,
    pub comments: Vec<Comment>,
    pub features: Vec<Feature>,
    pub keywords: Vec<Keyword>,
    pub references: Vec<Reference>,
    #[serde(rename = "uniProtKBCrossReferences")]
    pub uni_protkb_cross_references: Vec<CrossReference>,
    pub sequence: Sequence,
}

impl Entry {
    /// True for reviewed Swiss-Prot entries. Note the substring check must
    /// exclude `"unreviewed"` (TrEMBL), which contains `"reviewed"`.
    pub fn is_reviewed(&self) -> bool {
        let t = self.entry_type.to_lowercase();
        t.contains("reviewed") && !t.contains("unreviewed")
    }

    /// Recommended (or first available) protein name, e.g.
    /// `"Spike glycoprotein"`.
    pub fn protein_name(&self) -> Option<&str> {
        let pd = &self.protein_description;
        pd.recommended_name
            .as_ref()
            .and_then(|n| n.full_name.as_ref())
            .or_else(|| {
                pd.alternative_names
                    .iter()
                    .find_map(|n| n.full_name.as_ref())
            })
            .or_else(|| {
                pd.submission_names
                    .iter()
                    .find_map(|n| n.full_name.as_ref())
            })
            .map(|s| s.value.as_str())
    }

    /// Primary gene names (e.g. `["INS"]`); ORF names are not included.
    pub fn gene_names(&self) -> Vec<&str> {
        self.genes
            .iter()
            .filter_map(|g| g.gene_name.as_ref().map(|s| s.value.as_str()))
            .collect()
    }

    /// Concatenated FUNCTION comment text, if annotated.
    pub fn function_text(&self) -> Option<String> {
        let texts: Vec<&str> = self
            .comments
            .iter()
            .filter(|c| c.comment_type.eq_ignore_ascii_case("FUNCTION"))
            .flat_map(|c| c.texts.iter().map(|t| t.value.as_str()))
            .collect();
        if texts.is_empty() {
            None
        } else {
            Some(texts.join(" "))
        }
    }

    /// Cross-reference IDs for one database, e.g.
    /// `entry.xref_ids("Ensembl")` or `entry.xref_ids("PDB")`.
    pub fn xref_ids(&self, database: &str) -> Vec<&str> {
        self.uni_protkb_cross_references
            .iter()
            .filter(|x| x.database.eq_ignore_ascii_case(database))
            .map(|x| x.id.as_str())
            .collect()
    }
}

/// Version and date audit trail for an entry.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EntryAudit {
    pub first_public_date: Option<String>,
    pub last_annotation_update_date: Option<String>,
    pub last_sequence_update_date: Option<String>,
    pub entry_version: Option<u64>,
    pub sequence_version: Option<u64>,
}

/// Organism (or host) of an entry.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Organism {
    pub scientific_name: Option<String>,
    pub common_name: Option<String>,
    pub synonyms: Vec<String>,
    pub taxon_id: u64,
    pub lineage: Vec<String>,
}

/// A string value with its evidence tags — the workhorse of UniProt JSON.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EvidenceString {
    pub value: String,
    pub evidences: Vec<Evidence>,
}

/// One evidence tag (ECO code + source database reference).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Evidence {
    /// e.g. `"ECO:0000269"`.
    pub evidence_code: String,
    /// e.g. `"PubMed"`, `"HAMAP-Rule"`.
    pub source: Option<String>,
    /// ID within the source, e.g. a PMID.
    pub id: Option<String>,
}

/// The protein naming block (recommended / alternative / submission names).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProteinDescription {
    pub recommended_name: Option<NameBlock>,
    pub alternative_names: Vec<NameBlock>,
    /// Names submitted by sequence authors (TrEMBL entries).
    pub submission_names: Vec<NameBlock>,
    /// Names of proteolytic cleavage products.
    pub contains: Vec<ProteinDescription>,
    /// e.g. `"Fragment"` / `"Precursor"`.
    pub flag: Option<String>,
}

/// A full name plus optional short names and EC numbers.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NameBlock {
    pub full_name: Option<EvidenceString>,
    pub short_names: Vec<EvidenceString>,
    pub ec_numbers: Vec<EvidenceString>,
}

/// A gene and its name variants.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Gene {
    pub gene_name: Option<EvidenceString>,
    pub synonyms: Vec<EvidenceString>,
    pub orf_names: Vec<EvidenceString>,
}

/// A functional annotation comment (function, subunit, PTM, …).
///
/// The payload varies by `comment_type`: text-bearing comments populate
/// [`texts`](Self::texts), subcellular-location comments populate
/// [`subcellular_locations`](Self::subcellular_locations), and interaction
/// comments populate [`interactions`](Self::interactions).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Comment {
    /// e.g. `"FUNCTION"`, `"SUBCELLULAR LOCATION"`, `"INTERACTION"`.
    pub comment_type: String,
    /// Isoform / chain the comment applies to, when not the whole entry.
    pub molecule: Option<String>,
    pub texts: Vec<EvidenceString>,
    pub note: Option<CommentNote>,
    pub subcellular_locations: Vec<SubcellularLocation>,
    pub interactions: Vec<Interaction>,
}

impl Comment {
    /// The comment's text content with evidence tags stripped.
    pub fn text(&self) -> String {
        self.texts
            .iter()
            .map(|t| t.value.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Free-text note attached to a comment.
///
/// The API is inconsistent here: most notes are structured
/// (`{"texts": [...]}`) but `WEB RESOURCE` comments carry a bare string.
/// Both shapes deserialize into this type via an untagged enum.
#[derive(Debug, Clone, Default)]
pub struct CommentNote {
    pub texts: Vec<EvidenceString>,
}

impl<'de> Deserialize<'de> for CommentNote {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Plain(String),
            Structured {
                #[serde(default)]
                texts: Vec<EvidenceString>,
            },
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Plain(value) => Self {
                texts: vec![EvidenceString {
                    value,
                    evidences: Vec::new(),
                }],
            },
            Repr::Structured { texts } => Self { texts },
        })
    }
}

/// One localisation inside a SUBCELLULAR LOCATION comment.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubcellularLocation {
    pub location: Option<EvidenceString>,
    pub topology: Option<EvidenceString>,
    pub orientation: Option<EvidenceString>,
}

/// One interactant pair inside an INTERACTION comment.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Interaction {
    pub interactant_one: Interactant,
    pub interactant_two: Interactant,
    pub number_of_experiments: Option<u32>,
    pub organism_differ: Option<bool>,
}

/// One side of a protein–protein interaction.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Interactant {
    #[serde(rename = "uniProtKBAccession")]
    pub uni_prot_kb_accession: Option<String>,
    pub gene_name: Option<String>,
    pub int_act_id: Option<String>,
}

/// A sequence feature (signal peptide, domain, variant, …).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Feature {
    #[serde(rename = "type")]
    /// e.g. `"Signal"`, `"Domain"`, `"Natural variant"`.
    pub feature_type: String,
    pub location: Location,
    pub description: Option<String>,
    pub feature_id: Option<String>,
    pub evidences: Vec<Evidence>,
    /// Present on variant features: original → replacement amino acids.
    pub alternative_sequence: Option<AlternativeSequence>,
}

/// The amino-acid change carried by a variant feature.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlternativeSequence {
    pub original_sequence: Option<String>,
    pub alternative_sequences: Vec<String>,
}

/// Start/end coordinates of a feature.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Location {
    pub start: Option<Position>,
    pub end: Option<Position>,
}

/// A coordinate; `value` is absent for unknown positions.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Position {
    pub value: Option<u64>,
    /// e.g. `"EXACT"`.
    pub modifier: Option<String>,
}

/// A controlled vocabulary keyword.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Keyword {
    /// e.g. `"KW-0002"`.
    pub id: String,
    pub category: Option<String>,
    pub name: String,
}

/// A literature reference attached to an entry.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reference {
    pub reference_number: Option<u64>,
    pub citation: Option<Citation>,
    pub reference_positions: Vec<String>,
}

/// The citation of a literature reference.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Citation {
    pub citation_type: Option<String>,
    /// PubMed ID or DOI, depending on `citation_type`.
    pub id: Option<String>,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub journal: Option<String>,
    pub volume: Option<String>,
    pub first_page: Option<String>,
    pub last_page: Option<String>,
    pub publication_date: Option<String>,
    pub citation_cross_references: Vec<CrossReference>,
}

/// A cross-reference to a foreign database (EMBL, PDB, Ensembl, RefSeq, …).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CrossReference {
    /// Database name, e.g. `"PDB"`.
    pub database: String,
    /// ID within that database.
    pub id: String,
    /// Key/value pairs such as `ProteinId`, `MoleculeType`.
    pub properties: Vec<CrossRefProperty>,
    pub evidences: Vec<Evidence>,
}

/// One key/value property of a cross-reference.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CrossRefProperty {
    pub key: String,
    pub value: String,
}

/// The amino-acid sequence of an entry.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Sequence {
    pub value: String,
    pub length: Option<u64>,
    /// Molecular weight in Daltons.
    pub mol_weight: Option<u64>,
    pub crc64: Option<String>,
    pub md5: Option<String>,
}

// ===========================================================================
// Taxonomy
// ===========================================================================

/// A taxonomy node returned by the `/taxonomy` endpoints.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Taxon {
    pub taxon_id: u64,
    pub scientific_name: Option<String>,
    pub common_name: Option<String>,
    /// UniProt mnemonic, e.g. `"HUMAN"`.
    pub mnemonic: Option<String>,
    pub other_names: Vec<String>,
    pub parent: Option<TaxonNode>,
    /// e.g. `"species"`.
    pub rank: Option<String>,
    pub hidden: Option<bool>,
    pub active: Option<bool>,
    pub lineage: Vec<TaxonNode>,
}

impl Taxon {
    /// Best display name: scientific, falling back to common.
    pub fn name(&self) -> &str {
        self.scientific_name
            .as_deref()
            .or(self.common_name.as_deref())
            .unwrap_or("")
    }
}

/// A compact taxonomy node as it appears inside `parent` / `lineage`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaxonNode {
    pub taxon_id: u64,
    pub scientific_name: Option<String>,
    pub rank: Option<String>,
    pub hidden: Option<bool>,
}

// ===========================================================================
// Proteomes
// ===========================================================================

/// A reference proteome returned by the `/proteomes` endpoints.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Proteome {
    /// Proteome UPID, e.g. `"UP000005640"`.
    pub id: String,
    pub description: Option<String>,
    pub taxonomy: TaxonSummary,
    /// Last modification date (ISO).
    pub modified: Option<String>,
    /// e.g. `"Reference proteome"`.
    pub proteome_type: Option<String>,
    pub superkingdom: Option<String>,
    pub annotation_score: Option<u64>,
    pub gene_count: Option<u64>,
    pub protein_count: Option<u64>,
    pub proteome_statistics: ProteomeStatistics,
    pub genome_assembly: Option<GenomeAssembly>,
    pub taxon_lineage: Vec<TaxonNode>,
}

/// The organism summary embedded in a proteome.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaxonSummary {
    pub scientific_name: Option<String>,
    pub common_name: Option<String>,
    pub taxon_id: u64,
    pub mnemonic: Option<String>,
}

/// Reviewed/unreviewed protein counts of a proteome.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProteomeStatistics {
    pub reviewed_protein_count: Option<u64>,
    pub unreviewed_protein_count: Option<u64>,
    pub isoform_protein_count: Option<u64>,
}

/// The genome assembly a proteome is built from.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GenomeAssembly {
    /// e.g. `"GCF_000001405.40"`.
    pub assembly_id: Option<String>,
    /// e.g. `"chromosome"`, `"scaffold"`.
    pub level: Option<String>,
    pub source: Option<String>,
}

// ===========================================================================
// ID mapping
// ===========================================================================

/// Response of `POST /idmapping/run`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdMappingJob {
    pub job_id: Option<String>,
    pub errors: Vec<String>,
}

/// Response of `GET /idmapping/status/{jobId}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdMappingStatus {
    /// `"RUNNING"` or `"FINISHED"`.
    pub job_status: Option<String>,
    pub errors: Vec<String>,
}

impl IdMappingStatus {
    pub fn is_finished(&self) -> bool {
        self.job_status.as_deref() == Some("FINISHED")
    }

    pub fn is_running(&self) -> bool {
        self.job_status.as_deref() == Some("RUNNING")
    }
}

/// One row of an ID mapping result: source ID → mapped UniProtKB entry (or
/// `None` when the source ID had no mapping).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdMappingResult {
    pub from: String,
    pub to: Option<Entry>,
}

/// JSON envelope of the ID mapping results stream.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdMappingResults {
    pub results: Vec<IdMappingResult>,
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "entryType": "UniProtKB reviewed (Swiss-Prot)",
      "primaryAccession": "P01308",
      "uniProtkbId": "INS_HUMAN",
      "proteinDescription": {
        "recommendedName": {"fullName": {"value": "Insulin"}},
        "alternativeNames": [{"fullName": {"value": "Insulin A chain"}}]
      },
      "genes": [{"geneName": {"value": "INS"}}],
      "comments": [
        {"commentType": "FUNCTION", "texts": [{"value": "Insulin lowers blood glucose levels."}]},
        {"commentType": "SUBUNIT", "texts": [{"value": "Heterodimer."}]}
      ],
      "uniProtKBCrossReferences": [
        {"database": "PDB", "id": "1A7F"},
        {"database": "Ensembl", "id": "ENST00000397029"}
      ],
      "sequence": {"value": "MALWMRLL", "length": 110}
    }"#;

    #[test]
    fn entry_accessors() {
        let entry: Entry = serde_json::from_str(FIXTURE).expect("fixture parses");
        assert!(entry.is_reviewed());
        assert_eq!(entry.protein_name(), Some("Insulin"));
        assert_eq!(entry.gene_names(), vec!["INS"]);
        assert_eq!(
            entry.function_text().as_deref(),
            Some("Insulin lowers blood glucose levels.")
        );
        assert_eq!(entry.xref_ids("PDB"), vec!["1A7F"]);
        assert_eq!(entry.xref_ids("pdb"), vec!["1A7F"]);
        assert!(entry.xref_ids("EMBL").is_empty());
        assert_eq!(entry.sequence.length, Some(110));
    }

    #[test]
    fn entry_ignores_unknown_fields() {
        let json = FIXTURE.replace("\"length\": 110}", "\"length\": 110, \"novelField\": true}");
        let entry: Entry = serde_json::from_str(&json).expect("parses with unknown field");
        assert_eq!(entry.sequence.value, "MALWMRLL");
    }

    #[test]
    fn unreviewed_entry_defaults_hold() {
        // Minimal TrEMBL-shaped entry: no recommendedName, no comments.
        let entry: Entry = serde_json::from_str(
            r#"{"entryType": "UniProtKB unreviewed (TrEMBL)", "primaryAccession": "ABC123"}"#,
        )
        .expect("minimal entry parses");
        assert!(!entry.is_reviewed());
        assert_eq!(entry.protein_name(), None);
        assert_eq!(entry.function_text(), None);
        assert!(entry.gene_names().is_empty());
    }

    #[test]
    fn id_mapping_status_helpers() {
        let running: IdMappingStatus = serde_json::from_str(r#"{"jobStatus":"RUNNING"}"#).unwrap();
        assert!(running.is_running());
        assert!(!running.is_finished());

        let finished: IdMappingStatus =
            serde_json::from_str(r#"{"jobStatus":"FINISHED"}"#).unwrap();
        assert!(finished.is_finished());
    }

    #[test]
    fn format_roundtrip() {
        for f in [
            Format::Json,
            Format::Tsv,
            Format::List,
            Format::Fasta,
            Format::Gff,
            Format::Txt,
            Format::Xml,
        ] {
            assert_eq!(Format::parse(f.as_str()), Some(f));
        }
        assert_eq!(Format::parse(".fasta"), Some(Format::Fasta));
        assert_eq!(Format::parse("TSV"), Some(Format::Tsv));
        assert_eq!(Format::parse("nope"), None);
    }
}
