use serde::Serialize;

use crate::pb::v1 as pb;

/// Scores for one variant from one Atlas scorer (dense array decoded to `f32`).
#[derive(Debug, Clone, Serialize)]
pub struct VariantScoreArray {
    /// Scorer name, e.g. `AVI_SCORE`, `RNA_SEQ`, `AVI_SCORE_FEATURE_IMPORTANCE`.
    pub scorer: String,
    /// Whether the scorer's values are signed.
    pub is_signed: bool,
    /// Row-major shape of `values`.
    pub shape: Vec<usize>,
    /// Raw scores.
    pub values: Vec<f32>,
    /// Calibrated (quantile) scores, when the scorer is calibrated.
    pub calibrated_values: Option<Vec<f32>>,
    /// Track metadata aligned with the last dimension of `values`, if any.
    pub tracks: Vec<TrackInfo>,
    /// Gene metadata aligned with the last dimension of `values`, if any.
    pub genes: Vec<GeneInfo>,
}

/// All scores returned for one variant.
#[derive(Debug, Clone, Serialize)]
pub struct VariantScores {
    /// The variant as echoed back by the server.
    pub variant: VariantEcho,
    /// The interval the scores were computed over.
    pub interval: Option<IntervalEcho>,
    /// Per-scorer dense score arrays.
    pub scores: Vec<VariantScoreArray>,
}

/// A variant as reported by the server.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VariantEcho {
    pub chromosome: String,
    pub position: i64,
    pub reference_bases: String,
    pub alternate_bases: String,
}

impl From<pb::Variant> for VariantEcho {
    fn from(v: pb::Variant) -> Self {
        Self {
            chromosome: v.chromosome,
            position: v.position,
            reference_bases: v.reference_bases,
            alternate_bases: v.alternate_bases,
        }
    }
}

/// An interval as reported by the server (0-based half-open).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IntervalEcho {
    pub chromosome: String,
    pub start: i64,
    pub end: i64,
}

impl From<pb::Interval> for IntervalEcho {
    fn from(i: pb::Interval) -> Self {
        Self {
            chromosome: i.chromosome,
            start: i.start,
            end: i.end,
        }
    }
}

/// Human-readable track metadata.
#[derive(Debug, Clone, Serialize, Default)]
pub struct TrackInfo {
    pub name: String,
    pub assay: Option<String>,
    pub biosample: Option<String>,
    pub ontology_curie: Option<String>,
    pub transcription_factor: Option<String>,
    pub histone_mark: Option<String>,
}

/// Human-readable gene metadata.
#[derive(Debug, Clone, Serialize, Default)]
pub struct GeneInfo {
    pub gene_id: String,
    pub name: Option<String>,
    pub strand: Option<String>,
    pub junction_start: Option<i64>,
    pub junction_end: Option<i64>,
}

/// One output block of a model `ScoreVariant` call.
#[derive(Debug, Clone, Serialize)]
pub struct ScoredOutput {
    /// The scored variant.
    pub variant: VariantEcho,
    /// Shape of `values`.
    pub shape: Vec<usize>,
    /// Flat row-major scores.
    pub values: Vec<f32>,
    /// Track metadata aligned with the last dimension of `values`.
    pub tracks: Vec<TrackInfo>,
    /// Gene metadata aligned with the last dimension of `values`.
    pub genes: Vec<GeneInfo>,
}

/// Metadata for one Atlas scorer, from `ListVariantScoresMetadata`.
#[derive(Debug, Clone, Serialize)]
pub struct ScorerMetadataEntry {
    pub name: String,
    pub is_signed: bool,
    /// Number of values returned per variant for this scorer.
    pub num_values: usize,
    /// Track metadata for each value, when the scorer has track dimensions.
    pub tracks: Vec<TrackInfo>,
}

pub(crate) fn track_info(t: &pb::TrackMetadata) -> TrackInfo {
    TrackInfo {
        name: t.name.clone(),
        assay: t.assay.clone(),
        biosample: t.biosample.as_ref().map(|b| b.name.clone()),
        ontology_curie: ontology_curie(t.ontology_term.as_ref()),
        transcription_factor: t.transcription_factor_code.clone(),
        histone_mark: t.histone_mark_code.clone(),
    }
}

pub(crate) fn gene_info(g: &pb::GeneScorerMetadata) -> GeneInfo {
    GeneInfo {
        gene_id: g.gene_id.clone(),
        name: g.name.clone(),
        strand: g.strand.map(strand_label),
        junction_start: g.junction_start,
        junction_end: g.junction_end,
    }
}

fn strand_label(s: i32) -> String {
    match pb::Strand::try_from(s) {
        Ok(pb::Strand::Positive) => "+".to_owned(),
        Ok(pb::Strand::Negative) => "-".to_owned(),
        _ => ".".to_owned(),
    }
}

fn ontology_curie(term: Option<&pb::OntologyTerm>) -> Option<String> {
    let term = term?;
    let prefix = match pb::OntologyType::try_from(term.ontology_type) {
        Ok(pb::OntologyType::Clo) => "CLO",
        Ok(pb::OntologyType::Uberon) => "UBERON",
        Ok(pb::OntologyType::Cl) => "CL",
        Ok(pb::OntologyType::Efo) => "EFO",
        Ok(pb::OntologyType::Ntr) => "NTR",
        _ => return None,
    };
    Some(format!("{prefix}:{:07}", term.id))
}
