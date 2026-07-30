# ldsc

A pure-Rust port of [LD Score Regression](https://github.com/bulik/ldsc) (LDSC,
Bulik-Sullivan & Finucane 2015) — SNP-heritability (h²), genetic correlation
(rg), cell-type-specific analysis, LD-score computation from PLINK genotypes,
summary-statistic munging, and annotation building.

This crate is a faithful, line-by-line port of the canonical `/mnt/disk3/ldsc`
Python suite (≈ 6 000 lines) to idiomatic Rust. It is a **library only** — there
are no CLI binaries; every Python "argument" is a field on a config struct.
Numerics run on [`faer`](https://github.com/sarah-ek/faer) (no LAPACK/MKL
backend). Results are validated against the Python reference's own test suite,
ported to Rust, and its golden fixtures (vendored under `tests/data/`).

## Status

- **83 unit tests + 1 integration test, all passing.**
- `cargo clippy -p ldsc --all-targets -- -D warnings` is clean.
- Point estimates cross-checked bit-exact against a faithful Python3
  re-implementation on the `gwas-simulate` fixture (see
  [Validation](#validation)).

## What is ported

| Python source | Rust module | Contents |
| --- | --- | --- |
| `ldscore/regressions.py` | [`regress`](src/regress.rs) | `LD_Score_Regression` pipeline, `Hsq`, `Gencov`, `RG`, two-step estimator, liability conversion, `p_z_norm` |
| | [`hsq`](src/hsq.rs) | `estimate_h2` — the DataFusion `DataFrame` entry point (used by the TUI) |
| | [`irwls`](src/irwls.rs) | two-pass IRWLS + `Hsq.weights` / `Gencov.weights` |
| | [`jackknife`](src/jackknife.rs) | `LstsqJackknifeFast` + `RatioJackknife` |
| `ldscore/ldscore.py` | [`bedio`](src/bedio.rs) | PLINK `.bed` genotype reader + bit-counting MAF filter + `next_snps` |
| | [`ldscore`](src/ldscore.rs) | `getBlockLefts`, `block_left_to_right`, `corSumVarBlocks`, `.l2.ldscore` writer |
| `ldscore/parse.py` | [`io`](src/io.rs) | LDSC file-format readers (ldscore / M / annot / frq / cts / sumstats / bim / fam) |
| `ldscore/sumstats.py` | [`sumstats`](src/sumstats.rs) | `estimate_h2_from_files` driver, `check_variance`, allele filter/align, summary text |
| (new) | [`sldsc`](src/sldsc.rs) | S-LDSC per-annotation result table (`SldscResults`) + `.results` writer |
| `munge_sumstats.py` | [`munge`](src/munge.rs) | full column-mapping, filters, `process_n`, daner formats, `p_to_z` |
| `make_annot.py` | [`bed`](src/bed.rs) + [`make_annot`](src/make_annot.rs) | pure-Rust BED interval sort/merge/intersect; gene-set / BED → `.annot` |
| (scipy specials) | [`stats`](src/stats.rs) | `erfc`, `norm_ppf`, `chi2_sf` / `chi2_isf` (dependency-free) |
| (internal) | [`linalg`](src/linalg.rs), [`ingest`](src/ingest.rs) | faer WLS / SPD solve / condition number; `DataFrame` → vectors |
| | [`alleles`](src/alleles.rs) | `VALID_SNPS`, `MATCH_ALLELES`, `FLIP_ALLELES` |

## Dependencies

Only two crates are added beyond the workspace (`arrow`, `datafusion`, `faer`,
`thiserror`):

- `flate2` — gzip read **and** write (`.sumstats.gz`, `.l2.ldscore.gz`). Uses the
  pure-Rust `miniz_oxide` backend (no system zlib).
- `bzip2-rs` — bz2 **decompression only** (pure Rust, libbz2-free). LDSC outputs
  are always gzip, so there is no bz2 write path.

CSV/whitespace tokenising is done with `str::split_whitespace` (a ~30-line
helper in [`io`](src/io.rs)), which matches pandas' `delim_whitespace=True`
exactly and adds no dependency.

## Quick start

### h² from an already-joined DataFrame

```rust,no_run
use datafusion::prelude::SessionContext;
use ldsc::hsq::{HsqColumns, estimate_h2};

# async fn run(df: datafusion::prelude::DataFrame) -> ldsc::Result<()> {
let cols = HsqColumns {
    snp: "snp", z: "z", n: "n", ref_ld: vec!["ref_ld"], w_ld: "w_ld",
};
// `df` joins sumstats ⨝ ref_ld ⨝ w_ld on SNP, ordered by genomic position.
let res = estimate_h2(df, cols, &[160_554.6], 200, None).await?;
println!("h2 = {:.4} ± {:.4}", res.h2, res.h2_se);
println!("intercept = {:?}", res.intercept);
# Ok(())
# }
```

`estimate_h2` is aligned with Python's default: single-annotation + free
intercept → two-step estimator (cutoff 30); multiple annotations → `old_weights`.
Pass `intercept = Some(x)` for a constrained-intercept regression.

### h² directly from LDSC files

```rust,no_run
use ldsc::sumstats::{H2Config, NullLogger, estimate_h2_from_files};
let cfg = H2Config {
    sumstats: "trait.sumstats.gz".into(),
    ref_ld_chr: Some("ldscores/").into(),     // 22 per-chromosome files
    w_ld_chr: Some("wld/").into(),
    n_blocks: 200,
    ..Default::default()
};
let hsq = estimate_h2_from_files(&cfg, &mut NullLogger)?;
```

### S-LDSC (partitioned / stratified h²) from files

```rust,no_run
use ldsc::sumstats::{H2Config, NullLogger, estimate_sldsc};
let cfg = H2Config {
    sumstats: "trait.sumstats.gz".into(),
    ref_ld_chr: Some("baselineLD.").into(),   // multi-annotation (e.g. 97 cols)
    w_ld_chr: Some("weights.").into(),
    n_blocks: 200,
    ..Default::default()
};
let results = estimate_sldsc(&cfg, &mut NullLogger)?;
// results.annotations[k] = { category, coef, coef_se, coef_z, coef_p,
//   cat (per-annot h²), cat_se, enrichment, enrichment_se, enrichment_p, ... }
ldsc::sldsc::write_results(&results, "out.results")?;
```

### Munge a GWAS summary-statistics file

```rust,no_run
use ldsc::munge::{munge_sumstats, MungeConfig};
let m = munge_sumstats(&MungeConfig {
    sumstats: "gwas.txt.gz".into(),
    maf_min: 0.01, info_min: 0.9,
    ..Default::default()
})?;
ldsc::munge::write_sumstats_gz(&m, "trait.sumstats.gz")?;
```

### Compute LD scores from a PLINK `.bed`

```rust,no_run
use ldsc::bedio::PlinkBed;
use ldsc::io::BimFile;
use ldsc::ldscore::{get_block_lefts, ld_score_var_blocks, write_ldscore};

let bim = BimFile::read("ref.bim")?;
let mut bed = PlinkBed::read("ref.bed", n_indiv, &bim, None, None, 0.0)?;
let coords: Vec<f64> = (0..bed.m).map(|i| i as f64).collect(); // --ld-wind-snps
let bl = get_block_lefts(&coords, ld_window);
let out = ld_score_var_blocks(&mut bed, &bl, 50 /*chunk-size*/, None)?;
write_ldscore("out", &bed, &out.ld_score, &["L2".into()], &out.m_annot, &out.m_5_50)?;
```

### Build an annotation from a gene set

```rust,no_run
use ldsc::io::BimFile;
use ldsc::make_annot::{gene_set_to_annot, write_annot};
let bim = BimFile::read("ref.bim")?;
let annot = gene_set_to_annot("genes.txt", "ENSG_coord.txt", 100_000, &bim)?;
write_annot("out.annot", &annot)?;
```

## The estimators

**h² (univariate).** Regress per-SNP χ² on LD score:
`E[χ²ⱼ] = 1 + (Nⱼ/M)·h²·Lⱼ`. IRWLS weights + block jackknife (200 blocks) for
SEs. h² = Σₖ Mₖ·βₖ.

**Two-step estimator** (`regress::Hsq`, default for single-annotation + free
intercept). Step 1 fits the intercept on the low-χ² subset (χ² < 30) where
confounding dominates; step 2 fixes that intercept and fits h² on all SNPs.
More robust to high-χ² outliers than the joint fit. Implemented in
`regress::run_twostep` + `combine_twostep`.

**rg (bivariate).** `RG` runs `Hsq` on each trait and `Gencov` on their product
`z₁·z₂`, then a `RatioJackknife` of `gencov / √(h²₁·h²₂)`.

**S-LDSC (partitioned h²).** `estimate_sldsc` runs `Hsq` with `n_annot > 1`
(`old_weights`, `chisq_max = max(0.001·Nmax, 80)`), then
[`sldsc::build_sldsc_results`] derives per-annotation quantities:
`cat_k = M_k·β_k` (per-annot h²), `prop_k = cat_k/tot`,
`enrichment = prop_k / m_prop_k`, `enrichment_se = prop_se / m_prop_k`,
`enrichment_p` = two-sided test of enrichment ≠ 1.

**Liability scale.** `h2_obs_to_liab` / `gencov_obs_to_liab` convert observed-
scale estimates for binary traits given sample/population prevalence.

## S-LDSC reference panel

The baselineLD v2.2 reference panel (1000G EUR, 97 annotations) is archived at
`aliyun:autonomics-data/ldsc/s-ldsc-ref/`. Restore with:

```bash
rclone copy aliyun:autonomics-data/ldsc/s-ldsc-ref/ reference/ldsc_data/ -P
```

After restore, the file prefixes for `H2Config` / `read_ldscore` are:
- `ref_ld_chr`: `reference/ldsc_data/baselineLD.`
- `w_ld_chr`: `reference/ldsc_data/weights.hm3_noMHC.`
- `frqfile_chr`: `reference/ldsc_data/1000G.EUR.QC.`
- M_5_50 is read automatically from the `ref_ld_chr` prefix.

A golden BMI cross-validation output (`h²=0.2176`, from the Python LDSC suite)
is included at `test-bmi/xval_bmi.log`.

## File formats

Readers in [`io`](src/io.rs) handle, transparently gzipped / bzipped / plain:

- `.l2.ldscore` (per-chromosome `@`-substituted or single file), `.l2.M`,
  `.l2.M_5_50`
- `.annot` (overlap matrix `AᵀA`), `.frq`, `.cts`
- `.sumstats` (SNP/Z/N[/A1/A2], `.` = missing)
- PLINK `.bim` / `.fam`, filter files

Path helpers `sub_chr`, `get_present_chrs`, `which_compression` mirror the
Python exactly.

## Validation

The Python test suite (`test_parse.py`, `test_ldscore.py`, `test_regressions.py`,
`test_sumstats.py`, `test_munge_sumstats.py`) is ported to Rust and run against
**copies of the same fixtures** under `tests/data/`. Golden-value checks include:

- **PLINK `.bed`** — 8-SNP/5-indiv fixture → 4 polymorphic, `freq = [.6,.6,.625,.625]`,
  exact genotype bits, `next_snps` standardisation, `minor_ref` sign flip.
- **LD-score engine** — `cor_sum_var_blocks` matches an independent naïve r²
  computation (with and without annotation); `getBlockLefts` matches the Python
  triples.
- **Regressions** — `p_z_norm(10,1) → 1.523971e-23`; `h2_obs_to_liab(1,.5,.01)
  = 0.551907`; recovery of known partitioned h² (coef/cat/tot/prop/enrichment);
  `Gencov == Hsq` when z₁ = z₂; `RG ≈ −1`; negative-h² → NA.
- **Alleles** — the exact 32-element `MATCH_ALLELES` set matches Python.
- **Munge** — the end-to-end golden test reproduces `correct.sumstats`
  (SNP/A1/A2/Z/N, N = 6702 from a daner file).
- **Integration** — `estimate_h2` recovers h² on the `gwas-simulate` fixture
  (`tests/integration.rs`, run with `LDSC_RUN_INTEGRATION=1`).

### Cross-check against Python

`estimate_h2` was cross-checked against a faithful Python3 re-implementation of
LDSC's `Hsq` on the `gwas-simulate` fixture. Point estimates are bit-exact:

| estimator | h² | h² SE | intercept |
| --- | --- | --- | --- |
| Rust two-step (default) | 0.754280 | 0.0537 | 1.652062 |
| Python two-step | 0.754280 | 0.0537 | 1.652062 |
| Rust joint fit | 0.892615 | 0.054882 | 0.489255 |
| Python joint fit | 0.892615 | 0.054882 | 0.489255 |

> The two-step step-2 jackknife must use `update_separators`-mapped block
> boundaries (not uniform) — a throwaway Python verifier that forgot this once
> looked like a Rust bug.

## Special functions

`scipy.stats` quantities are implemented dependency-free in
[`stats`](src/stats.rs):

- `erfc` — Numerical Recipes `erfcc` (relative error < ~1.2e-7)
- `norm_ppf` — Acklam's rational approximation (absolute error < ~1.15e-9,
  refined by one Halley step)
- `chi2_sf(x, 1) = erfc(√(x/2))`, `chi2_isf(p, 1) = norm.isf(p/2)²`

## Known deviations from the Python suite

These are deliberate, documented choices:

- **Library only** — no `ldsc.py` / `munge_sumstats.py` / `make_annot.py` CLI
  binaries (per project decision); call the library functions instead.
- `estimate_rg_from_files` driver is not wired up, though `RG` + the allele
  match/align components exist and are tested.
- `io::read_annot` does not yet apply the 5–50 MAF filter from a `.frq` file.
- LD-score `cor_sum_var_blocks` computes the **exact** LD score (independent of
  `--chunk-size`); Python's default `c = 50` chunking is a performance
  approximation that can differ slightly near window boundaries.

## Running the tests

```bash
cargo test -p ldsc                        # unit + golden tests
LDSC_RUN_INTEGRATION=1 cargo test -p ldsc # + the gwas-simulate integration test
cargo clippy -p ldsc --all-targets -- -D warnings
```

The integration test points at `/mnt/disk3/gwas-simulate/output` by default
(override with `LDSC_FIXTURE_DIR`).
