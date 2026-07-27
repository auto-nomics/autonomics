//! Allele alignment — faithful port of `R/alignment.R`.
//!
//! Aligns the effect alleles of all sum-stats to the reference data set by
//! comparing allele-pair codes and sign-flipping where needed. Strand-ambiguous
//! / mismatched SNPs (code `NA`) are dropped.

/// Allele-pair code table, indexed by `(a1-1) + (a2-1)*4` for `a1,a2 ∈ {1..4}`
/// (A=1, C=2, G=3, T=4). `None` = strand-ambiguous / homozygous (unalignable).
/// Reproduces `pair.index$code` after `code[code > 10] = NA`.
const PAIR_CODE: [Option<i8>; 16] = [
    Some(0),    // (A,A) -> NA in R; we use a sentinel 0 = invalid here
    Some(-1),   // (C,A)
    Some(-2),   // (G,A)
    Some(0),    // (T,A) NA (was 11)
    Some(1),    // (A,C)
    Some(0),    // (C,C) NA
    Some(0),    // (G,C) NA (was 12)
    Some(2),    // (T,C)
    Some(2),    // (A,G)
    Some(0),    // (C,G) NA (was 12)
    Some(0),    // (G,G) NA
    Some(1),    // (T,G)
    Some(0),    // (A,T) NA (was 11)
    Some(-2),   // (C,T)
    Some(-1),   // (G,T)
    Some(0),    // (T,T) NA
];

/// Map a 1-letter allele to its numeric index (A=1,C=2,G=3,T=4), or 0 if not a
/// valid ACTG allele. Matches R `match(toupper(a), alleles)` returning `NA`.
#[inline]
fn allele_idx(a: &str) -> usize {
    match a.to_ascii_uppercase().as_str() {
        "A" => 1,
        "C" => 2,
        "G" => 3,
        "T" => 4,
        _ => 0,
    }
}

/// `map.alleles(a1, a2)`: allele-pair code for a single allele pair.
/// Returns `None` for invalid / strand-ambiguous pairs (R `NA`).
pub fn map_alleles(a1: &str, a2: &str) -> Option<i8> {
    let i = allele_idx(a1);
    let j = allele_idx(a2);
    if i == 0 || j == 0 {
        return None;
    }
    let code = PAIR_CODE[(i - 1) + (j - 1) * 4];
    if code == Some(0) {
        None
    } else {
        code
    }
}

/// `com.pair(pair1, pair2)`: compare two allele-pair codes.
/// `Some(1)` = same direction, `Some(-1)` = opposite direction, `None` =
/// mismatched / invalid (R `NA`).
#[inline]
pub fn com_pair(pair1: Option<i8>, pair2: Option<i8>) -> Option<f64> {
    match (pair1, pair2) {
        (Some(p1), Some(p2)) => {
            if p1 == p2 {
                Some(1.0)
            } else if p1.abs() == p2.abs() {
                Some(-1.0)
            } else {
                None
            }
        }
        _ => None,
    }
}
