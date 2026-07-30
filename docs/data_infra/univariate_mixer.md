# Univariate MiXeR Node: Algorithm & Data Infrastructure

[English](univariate_mixer.md) | [中文](univariate_mixer_zh.md)

> **Node type**: `univariate_mixer`  
> **Input port**: upstream GWAS sumstats (`Z: Float64, N: Float64, rsid: Utf8`)  
> **Output port**: single-row fit result DataFrame (pi, sig2_beta, sig2_zero, h2, nc, nc_p9, aic, bic, loglike)  
> **Depends on**: `iceberg.af.eur_af` (allele frequencies), `iceberg.mixer.eur_tagsuff` (precomputed sufficient statistics)

## 1. Mathematical model

### 1.1 Prior: spike-and-slab

Each SNP's effect size β_s follows a two-component mixture prior:

```
β_s ~ (1-π)·δ_0 + π·N(0, σ²_β)
```

Three free parameters:
| Parameter  | Description                                  | Constraint |
|------------|----------------------------------------------|------------|
| π          | Polygenicity (fraction of causal SNPs)       | (0, 1)     |
| σ²_β       | Causal effect-size variance (discoverability)| >0         |
| σ²_zero    | Inflation factor for the null component (intercept) | >0   |

### 1.2 Observation model: LD propagation

The z-score of each tag SNP j is the superposition of all causal SNPs propagated through LD:

```
z_j = Σ_s √(N_j · h_s · r²_{js}) · β_s + ε,   ε ~ N(0, σ²_zero)
```

where h_s = 2·maf_s·(1-maf_s) is the heterozygosity of SNP s, and r²_{js} is the LD between tag j and neighbor s.

### 1.3 Moment-matching Gaussian approximation

The exact marginal distribution of z_j is complex (the number of causal variants is uncertain).
MiXeR approximates it with a 2-component Gaussian mixture:

```
f(z_j) = tag_pi₀ · φ(z_j; 0, s₁²) + tag_pi₁ · φ(z_j; 0, s₂²)
```

The weights and variances of the two components (null and signal) are uniquely determined by
the 2nd moment A_j and 4th cumulant B_j of tag j's genetic effect:

```
A_j = E[δ_j²]   = π·σ²_β · Σ_s N_j · h_s · r²_{js}
B_j = κ₄(δ_j)   = 3π(1-π)(σ²_β)² · Σ_s (N_j · h_s · r²_{js})²
```

Closed-form solution:
```
tag_pi₀ = B / (B + 3A²)           ← null weight
tag_pi₁ = 1 − tag_pi₀             ← signal weight
σ²_tag  = (B + 3A²) / (3A)        ← signal excess variance
s₂      = √(σ²_zero + σ²_tag)     ← signal component std dev
```

## 2. Sufficient-statistics compression

### 2.1 Core insight

A_j and B_j factorize into **parameter part × data part**:

```
A_j = ebeta2 × m1_j    where ebeta2 = π·σ²_β,           m1_j = Σ_s N_j · h_s · r²_{js}
B_j = ebeta4 × m2_j    where ebeta4 = 3π(1-π)(σ²_β)²,  m2_j = Σ_s (N_j · h_s · r²_{js})²
```

m1_j and m2_j are pure-data scalars, independent of parameters. By **precomputing them once**,
each cost evaluation requires only O(1)/tag floating-point multiply-adds — no LD matrix access.

### 2.2 Why it matters

The optimizer (DE×20 rounds → Nelder-Mead refinement) calls the cost function tens of thousands
of times. The original implementation traverses the CSR neighbor table (`tag_moments`) on every
evaluation (O(nnz)/call), incurring massive total overhead.

After compression:
- LD scan goes from tens of thousands of times → 1 time (precompute)
- Memory collapses from O(nnz) CSR to O(n_snp) m1/m2/weights vectors
- The CSR can be released before fitting begins

### 2.3 Two-phase pipeline

```
┌──────────────────────────────────────────────────────────────────┐
│  Phase 1: precompute_tags (offline, per-chromosome)              │
│                                                                  │
│  af.eur_af ─→ h_vec (maf → 2·maf·(1-maf))                      │
│  ld_matrix.eur_chr{N} ─→ ld_pairs (r² ≥ 0.05)                  │
│                                                                  │
│  select_tags: MAF≥0.05 → LD prune (r²>0.8) → random subset      │
│                                                                  │
│  For each LD pair (a,b,r²):                                      │
│    if a ∈ tags: s1[a] += h_b·r²,  s2[a] += (h_b·r²)²           │
│    if b ∈ tags: s1[b] += h_a·r²,  s2[b] += (h_a·r²)²           │
│                                                                  │
│  weight = 1/(1 + Σr²)   ← inverse LD-score (de-redundancy)      │
│                                                                  │
│  Output: iceberg.mixer.eur_tagsuff                               │
│         columns: id_tag | s1 | s2 | sr | weight                 │
├──────────────────────────────────────────────────────────────────┤
│  Phase 2: univariate_mixer node (runtime)                        │
│                                                                  │
│  1. Read upstream sumstats → z_vec, n_vec, rsid→idx             │
│  2. Query af.eur_af → totalhet = Σ 2·maf·(1-maf), n_snp_ref    │
│  3. Read eur_tagsuff → after rsid matching:                      │
│       m1[idx] = N × s1     (note: s1 is N-free; multiply by N    │
│                                 at runtime)                      │
│       m2[idx] = N² × s2                                          │
│       weights[idx] = 1/(1+sr)                                    │
│       tags = [matched tag indices]                               │
│  4. fit1(suff): DE×repeats → NM → FitResult.derive()            │
│  5. Output single-row RecordBatch                                │
└──────────────────────────────────────────────────────────────────┘
```

### 2.4 N-free design

`s1`/`s2` are precomputed without N (N comes from GWAS sumstats, varying per-SNP). At runtime,
`N_tag` and `N_tag²` are injected:

```
m1[j] = n_vec[j] × s1[j]         ← s1[j] read from tagsuff table
m2[j] = n_vec[j]² × s2[j]        ← s2[j] read from tagsuff table
```

Benefit: the same tagsuff can be reused for GWAS with different sample sizes (no LD recompute
needed when N changes).

## 3. Cost function

### 3.1 Mathematical form

```
cost(π, σ²_β, σ²_zero) = -log L = Σ_{j∈tags} w_j · [-log f(z_j)]
```

where w_j is the de-redundancy weight (inverse LD-score), and f(z_j) is the moment-matched
Gaussian mixture density (see §1.3).

### 3.2 Implementation locations

| Function                  | File                              | Purpose                                       |
|---------------------------|-----------------------------------|-----------------------------------------------|
| `univariate_cost_gaussian`| `bio_crates/mixer/src/cost.rs`    | CSR reference impl (per-neighbor traversal)   |
| `univariate_cost_sufficient` | `bio_crates/mixer/src/cost.rs` | Compressed version (reads m1/m2), rayon parallel |
| `tag_moments`             | `bio_crates/mixer/src/cost.rs`    | Computes (A_j, B_j) for tag j, used by CSR   |

Validation: the `sufficient_cost_matches_gaussian` test confirms the two versions agree to < 1e-9
(ULP-level recombination).

### 3.3 Numerical safeguards

- `K_MIN_PDF = 1e-300`: prevents pdf underflow causing `log(0) = -∞`
- Skip when A=0 (tag with no LD signal), avoiding `tag_pi1 = 1 - B/(B+0)` and `sig2_tag` division by zero

## 4. Optimization strategy

### 4.1 Parameter transformation

The optimizer searches in an unconstrained space (with loose bounds), mapped to the parameter
space via bijection:

```
x₀ = ln(σ²_zero)     → σ²_zero > 0
x₁ = ln(σ²_β)        → σ²_β > 0
x₂ = logit(π)        → π ∈ (0,1),  logit(p) = ln(p/(1-p))
```

Search range (parameter space):
```
σ²_zero: [0.9, 2.5]
σ²_β:    [5e-6, 5e-2]
π:       [5e-5, 5e-1]
```

### 4.2 Two-stage optimization

```
fit1(data, cfg):
  1. Differential evolution (DE/rand/1/bin)
     - Population 45 (15×3 dims)
     - Per generation F∈[0.5,1.0] random, CR=0.7
     - Convergence: max_cost-min_cost < 0.01
     - Repeated cfg.diffevo_repeats times (default 20), different seed each time
     - Take global best

  2. Nelder-Mead refinement
     - Initial simplex edge length 0.5
     - Convergence: Δcost < 1e-7
     - Max 1200 iterations
     - Starts from DE best
```

Fully derivative-free — no gradients or Hessians needed.

## 5. Derived quantities

After fitting, derived quantities are computed from the optimal parameters + data:

| Quantity | Formula            | Description                                |
|----------|--------------------|--------------------------------------------|
| h²       | π·σ²_β·totalhet    | SNP heritability                           |
| nc       | π·n_snp            | Total number of causal variants            |
| nc_p9    | nc·0.319           | Number of causal variants explaining 90% h²|
| AIC      | 2·3 + 2·cost       | Akaike information criterion               |
| BIC      | ln(Σw)·3 + 2·cost  | Bayesian information criterion             |

Where `totalhet = Σ_s 2·maf_s·(1-maf_s)` (sum of heterozygosity over all reference-panel SNPs),
and `n_snp = |af.eur_af ∩ specified chromosomes|`.

**The 0.319 constant in nc_p9**: an empirically calibrated constant from the original C++.
Its meaning: the top 31.9% of causal variants (by effect size) explain 90% of heritability
(driven by the negative correlation between effect size and MAF under negative selection).
The current simplified version has not yet implemented the MAF-dependent effect-size model
(`sig2 ∝ maf^s`); the original constant is reused directly.

## 6. LD-score weighting

### 6.1 Motivation

Nearby tags share many neighbors through LD, making their z-scores highly correlated. If each
tag is counted independently in the likelihood, this is effectively **double-counting** the same
LD information.

### 6.2 Formula

```
w_j = 1 / (1 + Σ_s r²_{js})
```

| Σr² (tag's total LD score) | w_j  | Meaning                        |
|----------------------------|------|--------------------------------|
| ≈ 0                        | ≈ 1.0| Nearly independent             |
| 10                         | ≈ 0.09 | Highly redundant             |
| 100                        | ≈ 0.01 | Almost fully represented by neighbors |

LD-score weighting is a "soft" de-redundancy — smoother than randprune's "hard" boundary
(included/excluded), and requires only a single LD scan, no random sampling.
Statistically the two are equivalent (differ by 1–3%).

## 7. Tagsuff table schema

`iceberg.mixer.eur_tagsuff` (all chromosomes merged, EUR population):

| Column   | Type     | Description                                        |
|----------|----------|----------------------------------------------------|
| `id_tag` | Utf8     | rsid of the tag SNP                                |
| `s1`     | Float64  | N-free 1st-order sufficient statistic Σ h_neighbor·r² |
| `s2`     | Float64  | N-free 2nd-order sufficient statistic Σ (h_neighbor·r²)² |
| `sr`     | Float64  | Σ r² (for weight derivation; redundant but kept for diagnostics) |
| `weight` | Float64  | LdScore weight 1/(1+sr)                            |

One row per tag SNP (selected via MAF≥0.05 → LD prune r²>0.8 → random subset).
Non-tag SNPs are not in the table.

## 8. Key implementation decisions

### 8.1 Unified single table (current approach)

- `eur_tagsuff` is a single genome-wide Iceberg table (under the `iceberg.mixer` namespace).
- Produced by `precompute_tags` (`crates/data-engine/src/bin/precompute_tags.rs`) — **per-chromosome
  parquet files are generated and then merged and uploaded**.
- The node matches by rsid string (independent of chromosome number or integer index), so tagsuff
  and sumstats can have different index spaces.

### 8.2 N-free design

Runtime instantiation of `m1/m2`: tagsuff stores N-free `s1/s2`; the node injects per-SNP N at
runtime. This allows the same tagsuff to be reused for GWAS studies with different sample sizes.

### 8.3 Genome-wide scope

`eur_tagsuff` contains tags from all 22 autosomes. The node dynamically filters by sumstats rsid
intersection, so even though tagsuff has genome-wide tags, only the portion overlapping with
sumstats participates in the fit.

`totalhet` and `n_snp_ref` come from aggregate queries on `af.eur_af` for the chromosomes
specified in the spec — they must match the chromosome range of the sumstats, otherwise h² and nc
will be mis-scaled.

### 8.4 Simplification: sig2_zeroL = 0

The original MiXeR has a sig2_zeroL parameter capturing residual contributions from the low-r²
(< threshold) LD tail. The current implementation fixes sig2_zeroL=0. This is the main source of
the ~0.15% residual difference vs. the gold standard.

### 8.5 Merge vs. block-diagonal

With multiple chromosomes, the LD matrix is block-diagonal (no cross-chromosome LD). The original
MiXeR would `merge_blocks` to build a global CSR. In the current implementation:
- LdScore mode **does not need CSR** — LD is folded into m1/m2/sr in streaming batches, peak memory
  = one batch of a single chromosome.
- Tagsuff precompute replaces runtime CSR construction, further reducing peak memory to O(n_snp).

## 9. Known limitations

1. **EUR population only**: the `eur_tagsuff` name reflects that only EUR LD reference panels are
   currently available. Cannot be directly reused for cross-population GWAS.

2. **nc_p9 constant not precisely derived**: 0.319 comes from the original C++, calibrated on a
   negative-selection effect size-MAF model. The simplified version has not implemented this model,
   but nc_p9 only affects display, not fitting.

3. **Extract parameter / tagsuff consistency issue**: the node spec's `extract_*` fields are for
   logging/documentation only. The contents of the `eur_tagsuff` table are determined by hardcoded
   constants in the `precompute_tags` script (MAF=0.05, R2_PRUNE=0.8, SUBSET=2M, SEED=123).
   Modifying spec extract parameters without re-running precompute_tags will not change the tagsuff
   contents, leading to spec/data inconsistency.

4. **LdScore only**: tag weights are currently fixed to `1/(1+Σr²)`. The randprune weighting mode
   requires building a different tagsuff table (randprune weights need CSR + multiple random samples,
   cannot be folded into scalar weights).

5. **No mid-stream progress callback**: fit1's DE×NM is a synchronous CPU-intensive optimization
   with no `await` points. The node emits info events before and after execution to mark the time
   window, but cannot emit progress during optimization.

## 10. Code index

| Component                                           | Path                                                |
|-----------------------------------------------------|-----------------------------------------------------|
| Node definition + execute                           | `crates/data-engine/src/nodes/univariate_mixer.rs`  |
| tagsuff precompute script                           | `crates/data-engine/src/bin/precompute_tags.rs`     |
| Model parameters                                    | `bio_crates/mixer/src/params.rs`                    |
| Data containers (ChromData, UnivariateSufficient)   | `bio_crates/mixer/src/data.rs`                      |
| Cost function                                       | `bio_crates/mixer/src/cost.rs`                      |
| Optimizer (DE + NM)                                 | `bio_crates/mixer/src/optimizer.rs`                 |
| fit1 entry point                                    | `bio_crates/mixer/src/fit.rs`                       |
| Result derivation (h², nc, AIC…)                    | `bio_crates/mixer/src/result.rs`                    |
| Parameter mapping (log, logit)                      | `bio_crates/mixer/src/parametrize.rs`               |
| LD sparse matrix (CSR + BlockDiagonal)              | `bio_crates/mixer/src/ld_matrix.rs`                 |
| Tag selection (clumping)                            | `bio_crates/mixer/src/extract.rs`                   |
| Simulator (synthetic z-scores)                      | `bio_crates/mixer/src/simulate.rs`                  |
| Cross-validation vs original gold standard          | `bio_crates/mixer/tests/cross_validation.rs`        |
| E2E test (Iceberg GWAS)                             | `bio_crates/mixer/tests/test_univariate_mixer_e2e.rs` |
| Math derivation background                          | `bio_crates/mixer/README.md`                        |
