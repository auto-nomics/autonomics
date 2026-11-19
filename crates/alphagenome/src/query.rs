use std::str::FromStr;

use crate::error::{Error, Result};
use crate::pb::v1 as pb;

/// A single-nucleotide (or indel) variant, 1-based, GRCh38 for human.
///
/// Accepts the common spellings:
/// * AlphaGenome / portal: `chr22:36201698:A>C`
/// * gnomAD: `22-36201698-A-C`
/// * GTEx: `chr22_36201698_A_C_b38`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantQuery {
    /// Chromosome with the `chr` prefix normalised in (`chr22`).
    pub chromosome: String,
    /// 1-based position.
    pub position: i64,
    /// Reference bases.
    pub reference_bases: String,
    /// Alternate bases.
    pub alternate_bases: String,
}

impl VariantQuery {
    pub fn new(
        chromosome: impl Into<String>,
        position: i64,
        reference_bases: impl Into<String>,
        alternate_bases: impl Into<String>,
    ) -> Self {
        Self {
            chromosome: normalise_chromosome(&chromosome.into()),
            position,
            reference_bases: reference_bases.into(),
            alternate_bases: alternate_bases.into(),
        }
    }

    pub(crate) fn to_proto(&self) -> pb::Variant {
        pb::Variant {
            chromosome: self.chromosome.clone(),
            position: self.position,
            reference_bases: self.reference_bases.clone(),
            alternate_bases: self.alternate_bases.clone(),
        }
    }
}

fn normalise_chromosome(chrom: &str) -> String {
    let chrom = chrom.trim();
    if chrom.starts_with("chr") {
        chrom.to_owned()
    } else {
        format!("chr{chrom}")
    }
}

fn normalise_chromosome_mito(chrom: &str) -> String {
    let chrom = normalise_chromosome(chrom);
    // The portal/API spelling is chrM.
    if chrom.eq_ignore_ascii_case("chrMT") {
        "chrM".to_owned()
    } else {
        chrom
    }
}

impl FromStr for VariantQuery {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let (chrom, rest) = if let Some((a, b)) = s.split_once(':') {
            (a, b)
        } else if let Some((a, b)) = s.split_once('-') {
            // gnomAD: chr-pos-ref-alt
            (a, b)
        } else if let Some((a, b)) = s.split_once('_') {
            // GTEx: chr_pos_ref_alt[_b38] — only when the chromosome carries
            // a `chr` prefix, otherwise we cannot tell where it ends.
            if !a.eq_ignore_ascii_case("chr") && !a.to_ascii_lowercase().starts_with("chr") {
                return Err(Error::Parse(format!("unrecognised variant format: {s:?}")));
            }
            (a, b)
        } else {
            return Err(Error::Parse(format!(
                "unrecognised variant format: {s:?} (expected chr:pos:ref>alt)"
            )));
        };
        // GTEx: chr_pos_ref_alt[_b38] — no separator beyond '_'.
        let fields: Vec<&str> = if rest.contains('>') {
            let (pos, alleles) = rest
                .split_once(':')
                .ok_or_else(|| Error::Parse(format!("unrecognised variant format: {s:?}")))?;
            let (r, a) = alleles
                .split_once('>')
                .ok_or_else(|| Error::Parse(format!("unrecognised variant format: {s:?}")))?;
            vec![pos, r, a]
        } else {
            let separator = if rest.contains('_') { '_' } else { '-' };
            let mut parts: Vec<&str> = rest.split(separator).collect();
            // Strip an optional trailing build marker (`_b38`).
            if parts.len() > 3 && parts.last().is_some_and(|l| l.eq_ignore_ascii_case("b38")) {
                parts.pop();
            }
            if parts.len() != 3 {
                return Err(Error::Parse(format!("unrecognised variant format: {s:?}")));
            }
            parts
        };
        let position: i64 = fields[0]
            .parse()
            .map_err(|_| Error::Parse(format!("invalid position in {s:?}")))?;
        Ok(VariantQuery {
            chromosome: normalise_chromosome_mito(chrom),
            position,
            reference_bases: fields[1].to_ascii_uppercase(),
            alternate_bases: fields[2].to_ascii_uppercase(),
        })
    }
}

/// A genomic interval, 1-based closed for convenience (`chr11:5225727-5226575`),
/// converted to the API's 0-based half-open wire format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntervalQuery {
    pub chromosome: String,
    /// 1-based inclusive start.
    pub start: i64,
    /// 1-based inclusive end.
    pub end: i64,
}

impl IntervalQuery {
    pub fn new(chromosome: impl Into<String>, start: i64, end: i64) -> Self {
        Self {
            chromosome: normalise_chromosome_mito(&chromosome.into()),
            start,
            end,
        }
    }

    pub(crate) fn to_proto(&self) -> pb::Interval {
        pb::Interval {
            chromosome: self.chromosome.clone(),
            // 1-based closed -> 0-based half-open.
            start: self.start - 1,
            end: self.end,
            strand: pb::Strand::Unspecified as i32,
        }
    }
}

impl FromStr for IntervalQuery {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let (chrom, range) = s
            .split_once(':')
            .ok_or_else(|| Error::Parse(format!("unrecognised interval format: {s:?}")))?;
        let (start, end) = range
            .split_once('-')
            .ok_or_else(|| Error::Parse(format!("unrecognised interval format: {s:?}")))?;
        let start: i64 = start
            .trim()
            .parse()
            .map_err(|_| Error::Parse(format!("invalid interval start in {s:?}")))?;
        let end: i64 = end
            .trim()
            .parse()
            .map_err(|_| Error::Parse(format!("invalid interval end in {s:?}")))?;
        if end < start {
            return Err(Error::Parse(format!("interval end < start in {s:?}")));
        }
        Ok(IntervalQuery {
            chromosome: normalise_chromosome_mito(chrom),
            start,
            end,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alphagenome_spelling() {
        let v: VariantQuery = "chr22:36201698:A>C".parse().unwrap();
        assert_eq!(
            v,
            VariantQuery::new("chr22", 36201698, "A", "C")
        );
    }

    #[test]
    fn parses_without_chr_prefix_and_mito() {
        let v: VariantQuery = "22:36201698:A>C".parse().unwrap();
        assert_eq!(v.chromosome, "chr22");
        let m: VariantQuery = "MT:1000:A>G".parse().unwrap();
        assert_eq!(m.chromosome, "chrM");
    }

    #[test]
    fn parses_gnomad_spelling() {
        let v: VariantQuery = "22-36201698-A-C".parse().unwrap();
        assert_eq!(v, VariantQuery::new("chr22", 36201698, "A", "C"));
    }

    #[test]
    fn parses_gtex_spelling() {
        let v: VariantQuery = "chr22_36201698_A_C_b38".parse().unwrap();
        assert_eq!(v, VariantQuery::new("chr22", 36201698, "A", "C"));
    }

    #[test]
    fn parses_underscore_without_b38() {
        let v: VariantQuery = "chr1_100_A_T".parse().unwrap();
        assert_eq!(v, VariantQuery::new("chr1", 100, "A", "T"));
    }

    #[test]
    fn parses_interval() {
        let i: IntervalQuery = "chr11:5225727-5226575".parse().unwrap();
        assert_eq!(i, IntervalQuery::new("chr11", 5225727, 5226575));
        let p = i.to_proto();
        assert_eq!(p.start, 5225726);
        assert_eq!(p.end, 5226575);
    }

    #[test]
    fn rejects_garbage() {
        assert!("not a variant".parse::<VariantQuery>().is_err());
        assert!("chr1:5:AT".parse::<VariantQuery>().is_err());
        assert!("chr1:10-5".parse::<IntervalQuery>().is_err());
    }
}
