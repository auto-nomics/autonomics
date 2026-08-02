//! Allele / effect harmonisation — `R/harmonise.R`.
//!
//! Orients exposure and outcome effect alleles onto the same strand and effect
//! allele, flipping signs and allele frequencies as needed, and flags
//! palindromic / ambiguous / incompatible SNPs.
//!
//! # `action` — harmonisation strictness
//!
//! Three levels mirror `action` in `harmonise_data(action=…)`, captured by the
//! [`HarmoniseAction`] enum:
//! - [`HarmoniseAction::ForwardStrand`] (`1`) — assume forward strand (no
//!   palindrome inference). Only SNPs whose alleles cannot be reconciled by
//!   swap / strand-flip are dropped (`mr_keep = !remove`); palindromic SNPs are
//!   retained.
//! - [`HarmoniseAction::InferStrand`] (`2`, the default) — infer positive strand
//!   via allele frequencies. For a palindromic SNP whose exposure and outcome
//!   EAFs sit on opposite sides of 0.5, `beta_outcome` is sign-flipped and
//!   `eaf_outcome` complemented (`harmonise_22`); palindromic SNPs whose
//!   frequency is too close to 0.5 to call are flagged `ambiguous` and dropped
//!   (`mr_keep = !(remove || ambiguous)`).
//! - [`HarmoniseAction::ExcludePalindromic`] (`3`) — drop *all* palindromic SNPs
//!   regardless of frequency (`mr_keep = !(palindromic || remove ||
//!   ambiguous)`).
//!
//! # `tolerance` — allele-frequency band around 0.5
//!
//! `tolerance` (default `0.08`, see [`DEFAULT_TOLERANCE`]) is the half-width of
//! the "uninformative" EAF band centred on 0.5: `[0.5 − tolerance,
//! 0.5 + tolerance]` (i.e. `[0.42, 0.58]` by default). A SNP whose EAF falls
//! inside this band is considered **ambiguous** — its strand cannot be reliably
//! inferred from frequency alone. The same band also drives the `freq_similar`
//! checks in the `2-1` / `1-2` / `1-1` branches, where two GWAS's EAFs must
//! agree in direction (both below `minf`, both above `maxf`, or on opposite
//! sides) for the SNP to be considered unambiguous.
//!
//! Alleles use `Option<String>`: `None` is R's `NA`. A missing `other_allele`
//! selects the `2-1` / `1-2` / `1-1` harmonisation branch exactly as R's
//! `i22 / i21 / i12 / i11` split.
//
// The `status1` / `i` recomputations below mirror R's sequential
// re-assignment style verbatim; allow the resulting dead-store warnings.
#![allow(unused_assignments)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::MrError;

const DEFAULT_TOLERANCE: f64 = 0.08;

/// Harmonisation strictness — mirrors `action` in R's
/// `harmonise_data(action = …)`.
///
/// See the module-level docs for the full semantics of each variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarmoniseAction {
    /// Assume forward strand; keep palindromic SNPs (R `action = 1`).
    ForwardStrand,
    /// Infer strand from allele frequencies (the default, R `action = 2`).
    #[default]
    InferStrand,
    /// Drop all palindromic SNPs (R `action = 3`).
    ExcludePalindromic,
}

/// One exposure/outcome SNP pair to harmonise (already merged on SNP).
#[derive(Debug, Clone)]
pub struct HarmoniseInput {
    pub snp: String,
    pub id_exposure: String,
    pub id_outcome: String,
    pub beta_exposure: f64,
    pub beta_outcome: f64,
    pub se_exposure: f64,
    pub se_outcome: f64,
    /// Effect allele (exposure). `None` only valid if *both* are `None`; in
    /// practice exposure always has one.
    pub effect_allele_exposure: Option<String>,
    pub other_allele_exposure: Option<String>,
    pub effect_allele_outcome: Option<String>,
    pub other_allele_outcome: Option<String>,
    pub eaf_exposure: Option<f64>,
    pub eaf_outcome: Option<f64>,
}

/// Harmonised SNP row — `R/harmonise.R` output data frame columns.
#[derive(Debug, Clone)]
pub struct HarmoniseOutput {
    pub snp: String,
    pub id_exposure: String,
    pub id_outcome: String,
    pub beta_exposure: f64,
    pub beta_outcome: f64,
    pub se_exposure: f64,
    pub se_outcome: f64,
    pub effect_allele_exposure: String,
    pub other_allele_exposure: Option<String>,
    pub effect_allele_outcome: String,
    pub other_allele_outcome: Option<String>,
    pub eaf_exposure: Option<f64>,
    pub eaf_outcome: Option<f64>,
    pub remove: bool,
    pub palindromic: bool,
    pub ambiguous: bool,
    pub mr_keep: bool,
}

/// `check_palindromic(A1, A2)` — `R/harmonise.R:171`.
fn check_palindromic(a1: &str, a2: &str) -> bool {
    matches!((a1, a2), ("T", "A") | ("A", "T") | ("G", "C") | ("C", "G"))
}

/// `flip_alleles(x)` — `R/harmonise.R:179` (`chartr("ACGTacgt", "TGCAtgca")`).
fn flip_alleles(x: &str) -> String {
    x.chars()
        .map(|c| match c {
            'A' => 'T',
            'C' => 'G',
            'G' => 'C',
            'T' => 'A',
            'a' => 't',
            'c' => 'g',
            'g' => 'c',
            't' => 'a',
            other => other,
        })
        .collect()
}

fn toupper(s: &Option<String>) -> Option<String> {
    s.as_ref().map(|x| x.to_uppercase())
}

fn nchar(s: &str) -> usize {
    s.chars().count()
}

fn is_indel_marker(a1: &str, a2: &str) -> bool {
    nchar(a1) > 1 || nchar(a2) > 1 || a1 == "D" || a1 == "I"
}

// ---- recode_indels_22 — R/harmonise.R:186 (scalar) ----
fn recode_indels_22(
    a1: &str,
    a2: &str,
    b1: &str,
    b2: &str,
) -> (String, String, String, String, bool) {
    let nca1 = nchar(a1);
    let nca2 = nchar(a2);
    let ncb1 = nchar(b1);
    let ncb2 = nchar(b2);

    let (a1, a2, b1, b2) = recode_indels_22_body(a1, a2, b1, b2, nca1, nca2, ncb1, ncb2);

    let mut keep = true;
    if nca1 > 1 && nca1 == nca2 && (b1 == "D" || b1 == "I") {
        keep = false;
    }
    if ncb1 > 1 && ncb1 == ncb2 && (a1 == "D" || a1 == "I") {
        keep = false;
    }
    if a1 == a2 {
        keep = false;
    }
    if b1 == b2 {
        keep = false;
    }
    (a1, a2, b1, b2, keep)
}

#[allow(clippy::too_many_arguments)]
fn recode_indels_22_body(
    a1: &str,
    a2: &str,
    b1: &str,
    b2: &str,
    nca1: usize,
    nca2: usize,
    ncb1: usize,
    ncb2: usize,
) -> (String, String, String, String) {
    let (mut a1, mut a2, mut b1, mut b2) = (
        a1.to_string(),
        a2.to_string(),
        b1.to_string(),
        b2.to_string(),
    );

    if nca1 > nca2 && b1 == "I" && b2 == "D" {
        b1 = a1.clone();
        b2 = a2.clone();
    }
    if nca1 < nca2 && b1 == "I" && b2 == "D" {
        b1 = a2.clone();
        b2 = a1.clone();
    }
    if nca1 > nca2 && b1 == "D" && b2 == "I" {
        b1 = a2.clone();
        b2 = a1.clone();
    }
    if nca1 < nca2 && b1 == "D" && b2 == "I" {
        b1 = a1.clone();
        b2 = a2.clone();
    }
    if ncb1 > ncb2 && a1 == "I" && a2 == "D" {
        a1 = b1.clone();
        a2 = b2.clone();
    }
    if ncb1 < ncb2 && a1 == "I" && a2 == "D" {
        a2 = b1.clone();
        a1 = b2.clone();
    }
    if ncb1 > ncb2 && a1 == "D" && a2 == "I" {
        a2 = b1.clone();
        a1 = b2.clone();
    }
    if ncb1 < ncb2 && a1 == "D" && a2 == "I" {
        a1 = b1.clone();
        a2 = b2.clone();
    }
    (a1, a2, b1, b2)
}

// ---- recode_indels_21 — R/harmonise.R:234 (scalar) ----
fn recode_indels_21(
    a1: &str,
    a2: &str,
    b1: &str,
) -> (String, String, String, Option<String>, bool) {
    let (a1, a2) = (a1.to_string(), a2.to_string());
    let mut b1 = b1.to_string();
    let nca1 = nchar(&a1);
    let nca2 = nchar(&a2);
    let mut b2: Option<String> = None;

    if nca1 > nca2 && a1 != "I" && a1 != "D" && (b1 == "I" || b1 == "D") {
        // only meaningful for indels; literal R branches:
    }
    if nca1 > nca2 && b1 == "I" {
        let a1o = a1.clone();
        b1 = a1o;
        b2 = Some(a2.clone());
    }
    if nca1 < nca2 && b1 == "I" {
        b1 = a2.clone();
        b2 = Some(a1.clone());
    }
    if nca1 > nca2 && b1 == "D" {
        b1 = a2.clone();
        b2 = Some(a1.clone());
    }
    if nca1 < nca2 && b1 == "D" {
        b1 = a1.clone();
        b2 = Some(a2.clone());
    }

    let mut keep = true;
    if a1 == "I" && a2 == "D" {
        keep = false;
    }
    if a1 == "D" && a2 == "I" {
        keep = false;
    }
    if nca1 > 1 && nca1 == nca2 && (b1 == "D" || b1 == "I") {
        keep = false;
    }
    if a1 == a2 {
        keep = false;
    }
    (a1, a2, b1, b2, keep)
}

// ---- recode_indels_12 — R/harmonise.R:267 (scalar) ----
fn recode_indels_12(
    a1: &str,
    b1: &str,
    b2: &str,
) -> (String, Option<String>, String, String, bool) {
    let mut a1 = a1.to_string();
    let (b1, b2) = (b1.to_string(), b2.to_string());
    let ncb1 = nchar(&b1);
    let ncb2 = nchar(&b2);
    let mut a2: Option<String> = None;

    if ncb1 > ncb2 && a1 == "I" {
        a1 = b1.clone();
        a2 = Some(b2.clone());
    }
    if ncb1 < ncb2 && a1 == "I" {
        a2 = Some(b1.clone());
        a1 = b2.clone();
    }
    if ncb1 > ncb2 && a1 == "D" {
        a2 = Some(b1.clone());
        a1 = b2.clone();
    }
    if ncb1 < ncb2 && a1 == "D" {
        a1 = b1.clone();
        a2 = Some(b2.clone());
    }

    let mut keep = true;
    if b1 == "I" && b2 == "D" {
        keep = false;
    }
    if b1 == "D" && b2 == "I" {
        keep = false;
    }
    if ncb1 > 1 && ncb1 == ncb2 && (a1 == "D" || a1 == "I") {
        keep = false;
    }
    if b1 == b2 {
        keep = false;
    }
    (a1, a2, b1, b2, keep)
}

/// Internal per-row harmonised record (before mr_keep / id bookkeeping).
struct Row {
    snp: String,
    a1: String,
    a2: Option<String>,
    b1: String,
    b2: Option<String>,
    beta_a: f64,
    beta_b: f64,
    f_a: Option<f64>,
    f_b: Option<f64>,
    remove: bool,
    palindromic: bool,
    ambiguous: bool,
}

/// `harmonise_22` — `R/harmonise.R:300` (scalar).
#[allow(clippy::too_many_arguments)]
fn harmonise_22(
    snp: &str,
    a1: &str,
    a2: &str,
    b1: &str,
    b2: &str,
    beta_a: f64,
    mut beta_b: f64,
    f_a: Option<f64>,
    mut f_b: Option<f64>,
    tolerance: f64,
    action: HarmoniseAction,
) -> Row {
    let (mut a1, mut a2, mut b1, mut b2) = (
        a1.to_string(),
        a2.to_string(),
        b1.to_string(),
        b2.to_string(),
    );
    let mut remove_extra = false;
    if is_indel_marker(&a1, &a2) {
        let (na1, na2, nb1, nb2, keep) = recode_indels_22(&a1, &a2, &b1, &b2);
        a1 = na1;
        a2 = na2;
        b1 = nb1;
        b2 = nb2;
        if !keep {
            remove_extra = true;
        }
    }

    let mut status1 = a1 == b1 && a2 == b2;
    let mut to_swap = a1 == b2 && a2 == b1;
    if to_swap {
        std::mem::swap(&mut b1, &mut b2);
        beta_b = -beta_b;
        f_b = f_b.map(|f| 1.0 - f);
    }
    status1 = a1 == b1 && a2 == b2;
    let palindromic = check_palindromic(&a1, &a2);

    let mut i = !palindromic && !status1;
    if i {
        b1 = flip_alleles(&b1);
        b2 = flip_alleles(&b2);
    }
    status1 = a1 == b1 && a2 == b2;

    i = !palindromic && !status1;
    to_swap = a1 == b2 && a2 == b1;
    if to_swap {
        std::mem::swap(&mut b1, &mut b2);
        beta_b = -beta_b;
        f_b = f_b.map(|f| 1.0 - f);
    }
    status1 = a1 == b1 && a2 == b2;
    let mut remove = !status1;
    if remove_extra {
        remove = true;
    }

    let minf = 0.5 - tolerance;
    let maxf = 0.5 + tolerance;
    let tempfa = f_a.unwrap_or(0.5);
    let tempfb = f_b.unwrap_or(0.5);
    let ambig_a = tempfa > minf && tempfa < maxf;
    let ambig_b = tempfb > minf && tempfb < maxf;

    if action == HarmoniseAction::InferStrand {
        let status2 =
            ((tempfa < 0.5 && tempfb > 0.5) || (tempfa > 0.5 && tempfb < 0.5)) && palindromic;
        let to_swap = status2 && !remove;
        if to_swap {
            beta_b = -beta_b;
            f_b = f_b.map(|f| 1.0 - f);
        }
    }

    Row {
        snp: snp.to_string(),
        a1,
        a2: Some(a2),
        b1,
        b2: Some(b2),
        beta_a,
        beta_b,
        f_a,
        f_b,
        remove,
        palindromic,
        ambiguous: (ambig_a || ambig_b) && palindromic,
    }
}

/// `harmonise_21` — `R/harmonise.R:396` (scalar).
#[allow(clippy::too_many_arguments)]
fn harmonise_21(
    snp: &str,
    a1: &str,
    a2: &str,
    b1: &str,
    beta_a: f64,
    mut beta_b: f64,
    f_a: Option<f64>,
    mut f_b: Option<f64>,
    tolerance: f64,
    _action: HarmoniseAction,
) -> Row {
    let (mut a1, mut a2) = (a1.to_string(), a2.to_string());
    let mut b1 = b1.to_string();
    let mut b2: Option<String> = None;
    let mut ambiguous = false;
    let palindromic = check_palindromic(&a1, &a2);
    let mut remove = palindromic;
    let mut remove_extra = false;

    if is_indel_marker(&a1, &a2) {
        let (na1, na2, nb1, nb2, keep) = recode_indels_21(&a1, &a2, &b1);
        a1 = na1;
        a2 = na2;
        b1 = nb1;
        b2 = nb2;
        if !keep {
            remove_extra = true;
        }
    }
    if remove_extra {
        remove = true;
    }

    let mut status1 = a1 == b1;
    let minf = 0.5 - tolerance;
    let maxf = 0.5 + tolerance;
    let tempfa = f_a.unwrap_or(0.5);
    let tempfb = f_b.unwrap_or(0.5);
    let freq_similar1 = (tempfa < minf && tempfb < minf) || (tempfa > maxf && tempfb > maxf);
    if status1 && !freq_similar1 {
        ambiguous = true;
    }
    if status1 {
        b2 = Some(a2.clone());
    }

    let mut to_swap = a2 == b1;
    let freq_similar2 = (tempfa < minf && tempfb > maxf) || (tempfa > maxf && tempfb < minf);
    if to_swap && !freq_similar2 {
        ambiguous = true;
    }
    if to_swap {
        b2 = Some(b1.clone());
        b1 = a1.clone();
        beta_b = -beta_b;
        f_b = f_b.map(|f| 1.0 - f);
    }

    let to_flip = a1 != b1 && a2 != b1;
    if to_flip {
        ambiguous = true;
        b1 = flip_alleles(&b1);
    }
    status1 = a1 == b1;
    if status1 {
        b2 = Some(a2.clone());
    }

    to_swap = a2 == b1;
    if to_swap {
        b2 = Some(b1.clone());
        b1 = a1.clone();
        beta_b = -beta_b;
        f_b = f_b.map(|f| 1.0 - f);
    }

    Row {
        snp: snp.to_string(),
        a1,
        a2: Some(a2),
        b1,
        b2,
        beta_a,
        beta_b,
        f_a,
        f_b,
        remove,
        palindromic,
        ambiguous: ambiguous || palindromic,
    }
}

/// `harmonise_12` — `R/harmonise.R:476` (scalar).
#[allow(clippy::too_many_arguments)]
fn harmonise_12(
    snp: &str,
    a1: &str,
    b1: &str,
    b2: &str,
    mut beta_a: f64,
    mut beta_b: f64,
    mut f_a: Option<f64>,
    mut f_b: Option<f64>,
    tolerance: f64,
    _action: HarmoniseAction,
) -> Row {
    let mut a1 = a1.to_string();
    let (mut b1, mut b2) = (b1.to_string(), b2.to_string());
    let mut a2: Option<String> = None;
    let mut ambiguous = false;
    let palindromic = check_palindromic(&b1, &b2);
    let mut remove = palindromic;

    if nchar(&b1) > 1 || nchar(&b2) > 1 || b1 == "D" || b1 == "I" {
        let (na1, na2, nb1, nb2, keep) = recode_indels_12(&a1, &b1, &b2);
        a1 = na1;
        a2 = na2;
        b1 = nb1;
        b2 = nb2;
        if !keep {
            remove = true;
        }
    }

    let mut status1 = a1 == b1;
    let minf = 0.5 - tolerance;
    let maxf = 0.5 + tolerance;
    let tempfa = f_a.unwrap_or(0.5);
    let tempfb = f_b.unwrap_or(0.5);
    let freq_similar1 = (tempfa < minf && tempfb < minf) || (tempfa > maxf && tempfb > maxf);
    if status1 && !freq_similar1 {
        ambiguous = true;
    }
    if status1 {
        a2 = Some(b2.clone());
    }

    let mut to_swap = a1 == b2;
    let freq_similar2 = (tempfa < minf && tempfb > maxf) || (tempfa > maxf && tempfb < minf);
    if to_swap && !freq_similar2 {
        ambiguous = true;
    }
    if to_swap {
        a2 = Some(a1.clone());
        a1 = b1.clone();
        beta_a = -beta_a;
        f_a = f_a.map(|f| 1.0 - f);
    }

    let to_flip = a1 != b1 && a1 != b2;
    if to_flip {
        ambiguous = true;
        a1 = flip_alleles(&a1);
    }
    status1 = a1 == b1;
    if status1 {
        a2 = Some(b2.clone());
    }

    to_swap = b2 == a1;
    if to_swap {
        b2 = b1.clone();
        b1 = a1.clone();
        beta_b = -beta_b;
        f_b = f_b.map(|f| 1.0 - f);
    }

    Row {
        snp: snp.to_string(),
        a1,
        a2,
        b1,
        b2: Some(b2),
        beta_a,
        beta_b,
        f_a,
        f_b,
        remove,
        palindromic,
        ambiguous: ambiguous || palindromic,
    }
}

/// `harmonise_11` — `R/harmonise.R:558` (scalar).
#[allow(clippy::too_many_arguments)]
fn harmonise_11(
    snp: &str,
    a1: &str,
    b1: &str,
    beta_a: f64,
    beta_b: f64,
    f_a: Option<f64>,
    f_b: Option<f64>,
    tolerance: f64,
    _action: HarmoniseAction,
) -> Row {
    let a1 = a1.to_string();
    let b1 = b1.to_string();
    let status1 = a1 == b1;
    let remove = !status1;
    let palindromic = false;

    let minf = 0.5 - tolerance;
    let maxf = 0.5 + tolerance;
    let tempfa = f_a.unwrap_or(0.5);
    let tempfb = f_b.unwrap_or(0.5);
    let freq_similar1 = (tempfa < minf && tempfb < minf) || (tempfa > maxf && tempfb > maxf);
    let ambiguous = status1 && !freq_similar1;

    Row {
        snp: snp.to_string(),
        a1,
        a2: None,
        b1,
        b2: None,
        beta_a,
        beta_b,
        f_a,
        f_b,
        remove,
        palindromic,
        ambiguous: ambiguous || palindromic,
    }
}

/// Harmonise a single group (one exposure/outcome pair) — `R/harmonise.R:605`.
fn harmonise_group(rows: &[&HarmoniseInput], tolerance: f64, action: HarmoniseAction) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let a1 = toupper(&r.effect_allele_exposure);
        let a2 = toupper(&r.other_allele_exposure);
        let b1 = toupper(&r.effect_allele_outcome);
        let b2 = toupper(&r.other_allele_outcome);

        let a1u = a1.clone().unwrap_or_default();
        let a2u = a2.clone();
        let b1u = b1.clone().unwrap_or_default();
        let b2u = b2.clone();

        let row = match (a2.is_some(), b2.is_some()) {
            (true, true) => harmonise_22(
                &r.snp,
                &a1u,
                a2u.as_ref().unwrap(),
                &b1u,
                b2u.as_ref().unwrap(),
                r.beta_exposure,
                r.beta_outcome,
                r.eaf_exposure,
                r.eaf_outcome,
                tolerance,
                action,
            ),
            (true, false) => harmonise_21(
                &r.snp,
                &a1u,
                a2u.as_ref().unwrap(),
                &b1u,
                r.beta_exposure,
                r.beta_outcome,
                r.eaf_exposure,
                r.eaf_outcome,
                tolerance,
                action,
            ),
            (false, true) => harmonise_12(
                &r.snp,
                &a1u,
                &b1u,
                b2u.as_ref().unwrap(),
                r.beta_exposure,
                r.beta_outcome,
                r.eaf_exposure,
                r.eaf_outcome,
                tolerance,
                action,
            ),
            (false, false) => harmonise_11(
                &r.snp,
                &a1u,
                &b1u,
                r.beta_exposure,
                r.beta_outcome,
                r.eaf_exposure,
                r.eaf_outcome,
                tolerance,
                action,
            ),
        };
        out.push(row);
    }
    out
}

/// `harmonise_data(exposure_dat, outcome_dat, action, tolerance)` —
/// `R/harmonise.R:47`. The input slice is already merged on SNP (one record per
/// SNP per exposure/outcome pair); rows are grouped by `(id_exposure,
/// id_outcome)`, harmonised, then `mr_keep` is set from `action` and from
/// finite-ness of the four MR columns.
pub fn harmonise_data(inputs: &[HarmoniseInput]) -> Vec<HarmoniseOutput> {
    harmonise_data_with(inputs, HarmoniseAction::InferStrand, DEFAULT_TOLERANCE)
}

/// As [`harmonise_data`] with an explicit [`HarmoniseAction`] and
/// allele-frequency `tolerance` (default `0.08`).
///
/// # Parameters
///
/// - `action` — harmonisation strictness. See the module-level docs:
///   - [`HarmoniseAction::ForwardStrand`] — assume forward strand; keep
///     palindromic SNPs (`mr_keep = !remove`).
///   - [`HarmoniseAction::InferStrand`] — infer strand from allele frequencies;
///     drop ambiguous palindromic SNPs whose EAF is within `tolerance` of 0.5
///     (`mr_keep = !(remove || ambiguous)`).
///   - [`HarmoniseAction::ExcludePalindromic`] — drop *all* palindromic SNPs
///     (`mr_keep = !(palindromic || remove || ambiguous)`).
///
/// - `tolerance` — half-width of the uninformative EAF band `[0.5 − tolerance,
///   0.5 + tolerance]`. SNPs whose EAF falls inside this band cannot have their
///   strand inferred from frequency and are flagged `ambiguous`. Default
///   `0.08` → band `[0.42, 0.58]`.
///
/// In all actions, `mr_keep` is additionally forced to `false` when any of
/// `beta_exposure`, `beta_outcome`, `se_exposure`, or `se_outcome` is
/// non-finite.
pub fn harmonise_data_with(
    inputs: &[HarmoniseInput],
    action: HarmoniseAction,
    tolerance: f64,
) -> Vec<HarmoniseOutput> {
    // Group by (id_exposure, id_outcome) preserving first-seen order.
    let mut order: Vec<(String, String)> = Vec::new();
    let mut groups: std::collections::HashMap<(String, String), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, r) in inputs.iter().enumerate() {
        let key = (r.id_exposure.clone(), r.id_outcome.clone());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(i);
    }

    let mut out = Vec::new();
    for key in &order {
        let idxs = &groups[key];
        let refs: Vec<&HarmoniseInput> = idxs.iter().map(|&i| &inputs[i]).collect();
        let rows = harmonise_group(&refs, tolerance, action);
        for (row, r) in rows.into_iter().zip(refs.iter()) {
            let finite_betas = row.beta_a.is_finite()
                && row.beta_b.is_finite()
                && r.se_exposure.is_finite()
                && r.se_outcome.is_finite();
            let mut mr_keep = match action {
                HarmoniseAction::ExcludePalindromic => {
                    !(row.palindromic || row.remove || row.ambiguous)
                }
                HarmoniseAction::InferStrand => !(row.remove || row.ambiguous),
                HarmoniseAction::ForwardStrand => !row.remove,
            };
            if !finite_betas {
                mr_keep = false;
            }
            out.push(HarmoniseOutput {
                snp: row.snp,
                id_exposure: key.0.clone(),
                id_outcome: key.1.clone(),
                beta_exposure: row.beta_a,
                beta_outcome: row.beta_b,
                se_exposure: r.se_exposure,
                se_outcome: r.se_outcome,
                effect_allele_exposure: row.a1,
                other_allele_exposure: row.a2,
                effect_allele_outcome: row.b1,
                other_allele_outcome: row.b2,
                eaf_exposure: row.f_a,
                eaf_outcome: row.f_b,
                remove: row.remove,
                palindromic: row.palindromic,
                ambiguous: row.ambiguous,
                mr_keep,
            });
        }
    }
    out
}

// Suppress the unused-import lint for MrError (reserved for future validation).
#[allow(unused_imports)]
use MrError as _MrError;

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        snp: &str,
        ea_exp: &str,
        oa_exp: &str,
        ea_out: &str,
        oa_out: &str,
        b_exp: f64,
        b_out: f64,
        eaf_exp: f64,
        eaf_out: f64,
    ) -> HarmoniseInput {
        HarmoniseInput {
            snp: snp.to_string(),
            id_exposure: "exp".into(),
            id_outcome: "out".into(),
            beta_exposure: b_exp,
            beta_outcome: b_out,
            se_exposure: 0.01,
            se_outcome: 0.01,
            effect_allele_exposure: Some(ea_exp.into()),
            other_allele_exposure: Some(oa_exp.into()),
            effect_allele_outcome: Some(ea_out.into()),
            other_allele_outcome: Some(oa_out.into()),
            eaf_exposure: Some(eaf_exp),
            eaf_outcome: Some(eaf_out),
        }
    }

    #[test]
    fn matching_alleles_kept() {
        // Same alleles → kept, no sign flip.
        let r = row("rs1", "A", "G", "A", "G", 0.1, 0.2, 0.2, 0.2);
        let o = &harmonise_data(&[r])[0];
        assert!(o.mr_keep);
        assert!((o.beta_outcome - 0.2).abs() < 1e-12);
        assert_eq!(o.effect_allele_outcome, "A");
    }

    #[test]
    fn swapped_alleles_flip_sign() {
        // Outcome alleles reversed → beta flips, eaf complements.
        let r = row("rs1", "A", "G", "G", "A", 0.1, 0.2, 0.2, 0.3);
        let o = &harmonise_data(&[r])[0];
        assert!(o.mr_keep);
        assert!((o.beta_outcome - (-0.2)).abs() < 1e-12);
        assert!((o.eaf_outcome.unwrap() - 0.7).abs() < 1e-12);
    }

    #[test]
    fn palindromic_mid_freq_ambiguous_action2_dropped() {
        // A/T palindrome with freq ~0.5 → ambiguous → dropped under action 2.
        let r = row("rs1", "A", "T", "A", "T", 0.1, 0.2, 0.5, 0.5);
        let o = &harmonise_data(&[r])[0];
        assert!(o.palindromic);
        assert!(!o.mr_keep); // ambiguous under action 2
    }

    #[test]
    fn action1_keeps_palindromic() {
        let r = row("rs1", "A", "T", "A", "T", 0.1, 0.2, 0.5, 0.5);
        let o = &harmonise_data_with(&[r], HarmoniseAction::ForwardStrand, 0.08)[0];
        assert!(o.mr_keep);
    }

    #[test]
    fn incompatible_alleles_removed() {
        // A/G vs A/C — no strand flip or swap reconciles → removed.
        let r = row("rs1", "A", "G", "A", "C", 0.1, 0.2, 0.2, 0.2);
        let o = &harmonise_data(&[r])[0];
        assert!(o.remove);
        assert!(!o.mr_keep);
    }
}
