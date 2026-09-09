//! Sequence request options and sequence calculations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Sequence type accepted by Ensembl sequence endpoints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SequenceType {
    #[default]
    /// Genomic sequence.
    Dna,
    /// Spliced transcript sequence.
    Cdna,
    /// Coding sequence.
    Cds,
    /// Protein sequence.
    Protein,
}

impl SequenceType {
    /// API wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dna => "dna",
            Self::Cdna => "cdna",
            Self::Cds => "cds",
            Self::Protein => "protein",
        }
    }
}

/// Build query parameters shared by ID and region sequence requests.
pub(crate) fn sequence_params(
    sequence_type: SequenceType,
    expand_5prime: Option<u64>,
    expand_3prime: Option<u64>,
) -> Vec<(&'static str, String)> {
    let mut params = vec![("type", sequence_type.as_str().to_string())];
    if let Some(n) = expand_5prime {
        params.push(("expand_5prime", n.to_string()));
    }
    if let Some(n) = expand_3prime {
        params.push(("expand_3prime", n.to_string()));
    }
    params
}

/// Basic nucleotide statistics for a DNA string.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NucleotideStats {
    pub length: usize,
    pub a: usize,
    pub c: usize,
    pub g: usize,
    pub t: usize,
    pub n: usize,
    pub other: usize,
    pub gc_fraction: f64,
}

/// Compute case-insensitive nucleotide counts and GC fraction.
///
/// IUPAC ambiguity codes other than `N` are counted in `other`.
pub fn nucleotide_stats(seq: &str) -> NucleotideStats {
    let mut stats = NucleotideStats {
        length: seq.len(),
        a: 0,
        c: 0,
        g: 0,
        t: 0,
        n: 0,
        other: 0,
        gc_fraction: 0.0,
    };
    for base in seq.bytes().map(|b| b.to_ascii_uppercase()) {
        match base {
            b'A' => stats.a += 1,
            b'C' => stats.c += 1,
            b'G' => stats.g += 1,
            b'T' => stats.t += 1,
            b'N' => stats.n += 1,
            _ => stats.other += 1,
        }
    }
    let gc = stats.c + stats.g;
    stats.gc_fraction = if stats.length == 0 {
        0.0
    } else {
        gc as f64 / stats.length as f64
    };
    stats
}
