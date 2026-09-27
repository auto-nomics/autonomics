# Hypothesis-Testing Engine — Design

A comprehensive, composable hypothesis-testing system for the autonomics
data-engine DAG, modelled on R's **`hypothesize`** package
(<https://github.com/queelius/hypothesize>, v1.0.0) and extended to cover the
classical surface of R `stats::` for day-to-day tabular workflows.

The reference package is built on three SICP principles — **data
abstraction**, **closure** (combining tests yields tests), and
**higher-order functions** (transforming tests). We preserve all three in a
typed Rust core and re-expose them as DAG nodes whose output schema is itself
a *test row*, so any combinator node can sit downstream of any primitive.

Cross-validation target throughout: the corresponding R function from
`stats::`, `hypothesize::`, or `coin::`, at tolerance `1e-6` for p-values and
`1e-10` for statistics unless noted.

---

## 1. Goals & Non-Goals

### Goals

1. **Coverage** — every classical hypothesis test an analyst reaches for in
   R: z/t (one/two/paired), Wilcoxon/Mann–Whitney, proportion, variance,
   correlation, KS / Anderson–Darling / Shapiro, chi-squared (GoF and
   independence — the latter already exists as `chi_square`), Fisher exact,
   Kruskal–Wallis, Friedman, one-way ANOVA, Levene/Bartlett.
2. **Likelihood trinity** — Wald, LRT, Score as first-class nodes that accept
   fitted-model outputs (estimate ± SE, log-likelihoods, score + information)
   so regression nodes (`linear_regression`, `logistic_regression`,
   `cox_regression`, `susie_rss`, …) can feed them directly.
3. **Closure under combination** — Fisher's, Stouffer's, Tippett's, min-P,
   Wilkinson, and the Boolean algebra (intersection-union / union-intersection
   / complement) operate uniformly on the standard test-row schema, so any
   pipeline of tests can be combined into a single test.
4. **Multiple-testing correction** as a column-level transform — Bonferroni,
   Holm, Hochberg, Hommel, Benjamini–Hochberg, Benjamini–Yekutieli.
5. **Test–CI duality** — analytical CIs from Wald/z, plus a generic
   `invert_test` grid-search node that turns *any* scalar test node into a
   confidence interval.
6. **R parity** — every primitive is unit-tested against R on identical
   inputs (golden scenarios), mirroring the convention already used by
   `epi`, `ldsc`, `susie`, `lava`, etc.

### Non-Goals

- Bayesian hypothesis testing (covered by a future `bayes_tests` crate if
  needed; out of scope here).
- Permutation / bootstrap tests as a *generic engine* — we ship only the
  exact/asymptotic forms. A `permutation_test` node is sketched in §10 as
  future work.
- Re-implementing full model fitting inside this crate. We rely on
  `statkit::regression` and the existing regression nodes for that; the
  likelihood trinity nodes consume their outputs.

---

## 2. Architecture

Two layers, mirroring the established `statkit` / `epi` / node pattern:

```
┌─────────────────────────────────────────────────────────────┐
│  DAG nodes (crates/data-engine/src/nodes/hypothesize/*.rs)  │
│  - read columns OR scalar spec fields                       │
│  - emit / consume the standard "test row" schema            │
└───────────────────────┬─────────────────────────────────────┘
                        │ pure-Rust calls
┌───────────────────────▼─────────────────────────────────────┐
│  stat_crates/hypothesize (pure Rust, faer + statrs)         │
│  - HypothesisTest type + constructors                       │
│  - combinators, adjustments, invert                         │
│  - distribution helpers (CDF/inverse-CDF wrappers)          │
└─────────────────────────────────────────────────────────────┘
```

### 2.1 The `HypothesisTest` value type

Direct port of the S3 object from `hypothesize::hypothesis_test`. This is the
*typed algebraic core* — the crate API — independent of Arrow/DAG.

```rust
/// Alternative hypothesis, matching R's `alternative` argument everywhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, JsonSchema)]
pub enum Alternative { TwoSided, Less, Greater }

/// The fundamental data abstraction (SICP: data abstraction).
/// Every constructor returns one of these; every accessor reads from it.
#[derive(Clone, Debug)]
pub struct HypothesisTest {
    pub stat: f64,            // test statistic
    pub p_value: f64,         // p-value under H0
    pub dof: f64,             // degrees of freedom (f64::INFINITY for Normal)
    pub alternative: Alternative,
    pub method: &'static str, // e.g. "One Sample t-test"
    /// Free-form metadata mirrored from the R object (estimate, null_value,
    /// se, vcov, loglik, component_pvals, …). Stored as JSON for uniformity.
    pub extras: serde_json::Map<String, serde_json::Value>,
}

impl HypothesisTest {
    pub fn is_significant_at(&self, alpha: f64) -> bool { self.p_value < alpha }
}
```

Accessors `pval`, `test_stat`, `dof` are plain methods — no `UseMethod`
dispatch needed in Rust. The `extras` map absorbs everything R stored as
named list elements (`estimate`, `se`, `null_value`, `vcov`, `null_loglik`,
`alt_loglik`, `component_pvals`, `score`, `fisher_info`, `adjustment_method`,
`original_pval`, `n_tests`, …).

### 2.2 The standard "test row" schema (DAG contract)

This is the DAG-side dual of `HypothesisTest`: every primitive emits **one
row** with this schema (nullable columns omitted when not applicable):

| Column         | Type     | Always? | Meaning                                    |
|----------------|----------|---------|--------------------------------------------|
| `statistic`    | Float64  | yes     | test statistic                             |
| `p_value`      | Float64  | yes     | p-value                                    |
| `dof`          | Float64  | yes     | degrees of freedom (∞ for Normal tests)    |
| `alternative`  | Utf8     | yes     | `"two.sided" | "less" | "greater"`        |
| `method`       | Utf8     | yes     | human-readable method name                 |
| `estimate`     | Float64  | no      | point estimate (scalar tests)              |
| `null_value`   | Float64  | no      | hypothesised value under H0                |
| `conf_low`     | Float64  | no      | CI lower bound at `conf_level`             |
| `conf_high`    | Float64  | no      | CI upper bound                             |
| `conf_level`   | Float64  | no      | confidence level (default 0.95)            |
| `n`            | Int32    | no      | sample size                                |

Combinator nodes **consume** tables whose rows conform to this schema and
emit a single combined row, preserving the schema. This is the closure
property made operational in the DAG: `test → test`.

For multiple-testing correction the table is preserved row-for-row with one
extra column (`p_adj`) added, mirroring R's `p.adjust` semantics.

### 2.3 Module layout of `stat_crates/hypothesize`

```
stat_crates/hypothesize/
├── Cargo.toml                    # faer, statrs, serde, thiserror, schemars
├── src/
│   ├── lib.rs                    # HypothesisTest, Alternative, Result, errors
│   ├── dist.rs                   # wrappers: normal_cdf, t_sf, chisq_sf, f_sf, …
│   ├── primitives/
│   │   ├── mod.rs
│   │   ├── z.rs                  # z_test (one-sample / paired, known sigma)
│   │   ├── wald.rs               # wald_test: univariate (se) + multivariate (vcov)
│   │   ├── lrt.rs                # lrt(null_loglik, alt_loglik, dof)
│   │   ├── score.rs              # score_test (uni + multi)
│   │   ├── t.rs                  # t_test (one / two Welch / two pooled / paired)
│   │   ├── prop.rs               # prop_test (one / two, optional continuity corr)
│   │   ├── var.rs                # var_test (F), bartlett, levene, fligner
│   │   ├── ranks.rs              # wilcoxon, mann_whitney, kruskal_wallis, friedman
│   │   ├── gof.rs                # chisq_gof, shapiro, ks_one, ks_two, ad_test
│   │   ├── cor.rs                # cor_test (pearson / spearman / kendall)
│   │   ├── anova.rs              # oneway_anova (aov + Welch oneway.test)
│   │   └── table.rs              # fisher_exact (2×2, also r×c via network algorithm)
│   ├── combine/
│   │   ├── mod.rs
│   │   ├── fisher.rs             # Fisher's method     (-2 Σ log p)
│   │   ├── stouffer.rs           # Stouffer's Z        (Σ z_i/√k)
│   │   ├── tippett.rs            # Tippett's min-p / max-p
│   │   ├── boolean.rs            # intersection_test / union_test / complement_test
│   │   └── weighted.rs           # weighted Stouffer / weighted Fisher
│   ├── adjust.rs                 # adjust_pval (Bonferroni, Holm, Hochberg, Hommel, BH/BY, none)
│   └── invert.rs                 # invert_test + analytical CIs for wald/z
└── tests/
    ├── xval_stats.rs             # parity vs R `stats::` golden scenarios
    ├── xval_hypothesize.rs       # parity vs `hypothesize::` (same data/params)
    └── xval_coin.rs              # optional exact-test parity vs `coin::`
```

### 2.4 Module layout of DAG nodes

`crates/data-engine/src/nodes/hypothesize/mod.rs` re-exports factories; the
files register a `node_registry` entry each. To keep the registry tidy, all
`hypothesize::*` node kinds live under one sub-module.

```
crates/data-engine/src/nodes/hypothesize/
├── mod.rs                        # re-exports + TestRowSchema helpers
├── common.rs                     # TestRow builder, output schema, errors
├── t_test.rs
├── z_test.rs
├── wilcoxon.rs
├── prop_test.rs
├── var_test.rs
├── levene_test.rs
├── bartlett_test.rs
├── anova_test.rs
├── kruskal_wallis.rs
├── friedman_test.rs
├── cor_test.rs
├── ks_test.rs
├── shapiro_test.rs
├── ad_test.rs
├── chisq_gof.rs
├── fisher_exact.rs
├── wald_test.rs
├── lrt.rs
├── score_test.rs
├── combine_pvalues.rs
├── boolean_test.rs
├── adjust_pvalues.rs
└── invert_test.rs
```

Each node registers under `node_kind = "hypothesize.<test>"` (e.g.
`"hypothesize.t_test"`) so the namespace stays collision-free with the
existing top-level `chi_square` and `linear_regression` nodes. The
`chi_square` independence node is *not* renamed — the new `chisq_gof` node
complements it.

---

## 3. The `HypothesisTest` Algebraic Core (crate)

The crate faithfully ports every function exported by `hypothesize`'s
NAMESPACE, then extends it. Each function below lists its R signature, the
Rust signature, and any deviations.

### 3.1 Likelihood trinity (ports of `hypothesize::`)

| R function                                  | Rust constructor                                              | dof rule                                                   |
|---------------------------------------------|---------------------------------------------------------------|------------------------------------------------------------|
| `lrt(null_loglik, alt_loglik, dof)`         | `lrt(null_loglik, alt_loglik, dof)`                           | chi-sq dof                                                 |
| `wald_test(estimate, se=, vcov=, null=)`    | `wald_uni(estimate, se, null)`, `wald_multi(estimate, vcov, null)` | uni → χ²(1); multi → χ²(k)                          |
| `z_test(x, mu0, sigma, alternative)`        | `z_test(&x, mu0, sigma, alt)`                                 | ∞ (Normal)                                                 |
| `score_test(score, fisher_info, null)`      | `score_uni(score, fi, null)`, `score_multi(&score, &fi, null)` | χ²(1) / χ²(k)                                         |

The R `hypothesize::wald_test` collapses both cases into one function via
optional args; we split them in Rust for type-safety (a `vcov` matrix needs
`&[Vec<f64>]` / `faer::Mat`). The trinity outputs are interchangeable in
combinators — same `HypothesisTest` type.

The LRT function accepts `logLik` objects in R; in Rust we accept a small
`LogLik { loglik: f64, df: u32 }` struct and auto-derive `dof = alt.df −
null.df` when both are present, matching the R behaviour.

### 3.2 Combinators (ports of `hypothesize::`)

| R function                       | Rust function                                   | p-value rule             |
|----------------------------------|-------------------------------------------------|--------------------------|
| `fisher_combine(...)`            | `fisher_combine(&[&HypothesisTest])`            | χ²(2k) tail of −2 Σ log p |
| `intersection_test(...)`         | `intersection_test(&[&HypothesisTest])`         | max(p)                   |
| `union_test(...)`                | `union_test(&[&HypothesisTest])`                | min(p)                   |
| `complement_test(t)`             | `complement_test(&t)`                           | 1 − p                    |

De Morgan's law holds by construction (we test it directly):
`union(a,b) ≡ complement(intersection(complement(a), complement(b)))`.

### 3.3 Higher-order functions

| R function                              | Rust function                                              | Notes                                                    |
|-----------------------------------------|------------------------------------------------------------|----------------------------------------------------------|
| `adjust_pval(x, method, n)`             | `adjust_pvals(&mut [&mut HypothesisTest], method, n)`      | delegates to `p_adjust_raw(&[f64], method, n)`           |
| `confint(wald_test/z_test, level)`      | `confint_wald(&t, level)`, `confint_z(&t, level)`          | analytical, one-sided vs two-sided alt handling          |
| `invert_test(test_fn, grid, alpha)`     | `invert_test(FnMut(f64) -> HypothesisTest, &grid, alpha)`  | returns `ConfidenceSet { set, alpha, level, grid }`      |

### 3.4 Extensions beyond `hypothesize`

To cover the practical R `stats::` surface, the crate adds sample-data
primitives. Each returns a `HypothesisTest` and feeds the same combinators.
Signatures are deliberately R-aligned so cross-validation code reads nearly
identically to the R reference.

```rust
// t_test — matches R t.test.default
pub fn t_test_one (x: &[f64], mu0: f64, alt: Alternative) -> HypothesisTest;
pub fn t_test_two (x: &[f64], y: &[f64],
                   paired: bool, var_equal: bool,
                   alt: Alternative) -> HypothesisTest;

// prop_test — matches R prop.test
pub fn prop_test_one (x: u32, n: u32, p0: f64,
                      alt: Alternative, correct: bool) -> HypothesisTest;
pub fn prop_test_two(x1: u32, n1: u32, x2: u32, n2: u32,
                     alt: Alternative, correct: bool) -> HypothesisTest;

// var_test / bartlett / levene / fligner
pub fn var_test    (x: &[f64], y: &[f64], ratio: f64, alt: Alternative) -> HypothesisTest;
pub fn bartlett_test(groups: &[&[f64]]) -> HypothesisTest;
pub fn levene_test (groups: &[&[f64]], center: LeveneCenter) -> HypothesisTest; // Mean | Median(Brown-Forsythe)
pub fn fligner_test(groups: &[&[f64]]) -> HypothesisTest;

// ranks
pub fn wilcoxon_signed_rank(x: &[f64], mu0: f64, alt: Alternative,
                            zero_method: ZeroMethod, correct: bool) -> HypothesisTest;
pub fn mann_whitney      (x: &[f64], y: &[f64], alt: Alternative,
                          correct: bool) -> HypothesisTest;
pub fn kruskal_wallis    (groups: &[&[f64]]) -> HypothesisTest;
pub fn friedman_test     (blocks: &[&[f64]]) -> HypothesisTest;

// ANOVA
pub fn oneway_anova      (groups: &[&[f64]], var_equal: bool) -> HypothesisTest;

// GoF / distributions
pub fn chisq_gof         (observed: &[u64], expected: &[f64], p: &[f64],
                          rescale_p: bool) -> HypothesisTest;
pub fn shapiro_wilk      (x: &[f64]) -> HypothesisTest;       // Royston algorithm AS R94
pub fn ks_one_sample     (x: &[f64], cdf: impl Fn(f64)->f64) -> HypothesisTest;
pub fn ks_two_sample     (x: &[f64], y: &[f64], alt: Alternative) -> HypothesisTest;
pub fn anderson_darling  (x: &[f64], dist: AdDist) -> HypothesisTest; // Normal | Exponential

// correlation
pub fn cor_test          (x: &[f64], y: &[f64], method: CorMethod, alt: Alternative) -> HypothesisTest;

// contingency
pub fn fisher_exact      (matrix: &[[u32;2];2], alt: Alternative,
                          conf_level: f64) -> HypothesisTest;
```

### 3.5 Distribution helpers (`dist.rs`)

Thin wrappers over `statrs` to give us the R-named tail functions used
throughout (`pnorm`, `pt`, `pchisq`, `pf`, `pbinom`, `pnhyper`, `pswilk`,
`pks`, `padnorm`). Two-sided p-values use `2 * min(cdf, sf)` consistently.

### 3.6 Multiple-testing correction (`adjust.rs`)

Direct port of `stats::p.adjust` (the same C algorithm R uses). The seven
methods are: `bonferroni`, `holm`, `hochberg`, `hommel`, `bh` (= `fdr`),
`by`, `none`. `Hommel` is the only non-trivial one (its set-based step-down
logic is ported verbatim from R's C source and unit-tested against the same
vectors). A `n_total` override mirrors R's `n` argument for partial families.

### 3.7 Inversion & CI (`invert.rs`)

`invert_test` is a higher-order function: it takes a closure `FnMut(f64) ->
HypothesisTest` and a grid, returning the set of non-rejected null values.
Analytical `confint_wald` / `confint_z` shortcuts are provided for the
common case so we don't pay for a grid search when the maths is closed-form.
The one-sided vs two-sided alternative branch matches the R S3 methods
exactly (one-sided intervals have an `±∞` bound).

---

## 4. DAG Node Catalogue

Each entry: **kind** · R reference · input contract · output contract ·
spec fields.

### 4.1 Group A — Sample-data primitives (table → one test row)

All nodes in this group:
- **Inputs**: one port (`port_0`); the dataset.
- **Output**: one row conforming to §2.2's *standard test row schema*.
- **Shared spec fields** (in every Group A spec):
  - `alternative: "two.sided" | "less" | "greater"` (default `"two.sided"`)
  - `conf_level: f64` (default `0.95`)

| Kind                          | R ref                     | Reads                                                | Extra spec fields                          |
|-------------------------------|---------------------------|------------------------------------------------------|--------------------------------------------|
| `hypothesize.t_test`          | `t.test`                  | `x_column`; optional `y_column`, `group_column` for two-sample (long format); `paired: bool`, `var_equal: bool`, `mu: f64` | as listed |
| `hypothesize.z_test`          | `hypothesize::z_test`     | `x_column`, `mu: f64`, `sigma: f64`                  | —                                          |
| `hypothesize.wilcoxon`        | `wilcox.test`             | `x_column`; optional `y_column`/`group_column`; `paired`, `mu`, `correct`, `zero_method` | —             |
| `hypothesize.prop_test`       | `prop.test`               | `count_column`+`n_column` (one or two rows), or `x_column`+`n_scalar`; `p: f64`, `correct: bool` | —                |
| `hypothesize.var_test`        | `var.test`                | `x_column`, `y_column` or `group_column`; `ratio: f64` | —                                        |
| `hypothesize.levene_test`     | `car::leveneTest`         | `value_column`, `group_column`; `center: "mean"\|"median"` | —                                    |
| `hypothesize.bartlett_test`   | `bartlett.test`           | `value_column`, `group_column`                       | —                                          |
| `hypothesize.fligner_test`    | `fligner.test`            | `value_column`, `group_column`                       | —                                          |
| `hypothesize.oneway_anova`    | `aov` / `oneway.test`     | `value_column`, `group_column`; `var_equal: bool`    | —                                          |
| `hypothesize.kruskal_wallis`  | `kruskal.test`            | `value_column`, `group_column`                       | —                                          |
| `hypothesize.friedman_test`   | `friedman.test`           | `value_column`, `group_column`, `block_column`       | —                                          |
| `hypothesize.cor_test`        | `cor.test`                | `x_column`, `y_column`; `method: "pearson"\|"spearman"\|"kendall"` | —                  |
| `hypothesize.ks_test`         | `ks.test`                 | `x_column`; optional `y_column` (two-sample) or `dist: "norm"\|"unif"\|"exp"` + params; `exact: bool` | — |
| `hypothesize.shapiro_test`    | `shapiro.test`            | `x_column` (3 ≤ n ≤ 5000)                            | —                                          |
| `hypothesize.ad_test`         | `nortest::ad.test` etc.   | `x_column`; `dist: "norm"\|"exp"`                    | —                                          |
| `hypothesize.chisq_gof`       | `chisq.test` (GoF)        | `count_column`; `p_column` or `p: [f64]`; `rescale_p: bool` | —                              |
| `hypothesize.fisher_exact`    | `fisher.test`             | `row_column`, `col_column` (must be 2×2); or `matrix` literal | `or: "two.sided"\|"greater"\|"less"`; `conf_level` |

The existing `chi_square` node covers the `chisq.test(table(row, col))`
independence case; `chisq_gof` covers the goodness-of-fit case
(`chisq.test(x, p=…)`).

#### Output schema additions per node

Most Group A nodes also emit a few method-specific columns alongside the
standard row (mirroring R's `htest` object): `estimate1`, `estimate2`,
`stderr`, `df1`/`df2` (for F-tests), `numerator_df`, `denominator_df`,
`method_details` (Utf8). These live in `extras` in the crate and are
promoted to typed columns by the node layer.

### 4.2 Group B — Likelihood trinity (scalars/vectors → test row)

These nodes read **scalars and small matrices from the spec**, not from
columns. They are the natural sink for fitted-model outputs: the agent (or
the DAG author) wires a `linear_regression`/`logistic_regression`/
`cox_regression` output through a tiny projection node (or reads from a
single-row upstream table) and feeds the values into these specs.

| Kind                          | Spec fields                                                                          | Output                       |
|-------------------------------|--------------------------------------------------------------------------------------|------------------------------|
| `hypothesize.wald_test`       | `estimate: number \| [number]`, `se: number` **or** `vcov: [[number]]`, `null_value: number \| [number]` (default 0) | standard row, `dof` = 1 or k |
| `hypothesize.lrt`             | `null_loglik: number`, `alt_loglik: number`, `dof: int` **or** `null_df`+`alt_df`    | standard row                 |
| `hypothesize.score_test`      | `score: number \| [number]`, `fisher_info: number \| [[number]]`, `null_value?: number \| [number]` | standard row    |

R-parity subtlety: the R `wald_test` reports the χ² statistic and stores the
z-score separately; we mirror this — `statistic` column = χ², `extras.z` =
z (univariate case only).

### 4.3 Group C — Combinators (k test rows → 1 test row)

These operate on the **`p_value` column of their input port** (each row =
one component test). They are the DAG embodiment of the closure property.

| Kind                            | R ref                              | Spec                                                            | Output                                                          |
|---------------------------------|------------------------------------|-----------------------------------------------------------------|-----------------------------------------------------------------|
| `hypothesize.combine_pvalues`   | `poolr::fisher / stouffer / tippett` | `method: "fisher"\|"stouffer"\|"tippett_min"\|"tippett_max"\|"wilkinson"`, optional `weights_column`, optional `effect_column`+`var_column` (Stouffer) | one combined test row                                            |
| `hypothesize.boolean_test`      | `hypothesize::intersection_test/union_test/complement_test` | `op: "intersection"\|"union"\|"complement"`, `alpha: f64` (for complement inversion) | one combined row; `statistic`/`dof` are null for and/or        |
| `hypothesize.adjust_pvalues`    | `stats::p.adjust`                  | `method`, optional `n_total: int`, optional `p_column: "p_value"` (default), optional `alpha: 0.05` (default) | **passthrough** — same rows + `p_adj` column + `reject` column (`p_adj < alpha`) |

`adjust_pvalues` is the **only** non-closure combinator — it preserves the
row count because it's a column transform, not a test aggregator. This is
intentional and matches R semantics.

`combine_pvalues` weights:
- Fisher's method: statistic `−2 Σ wᵢ log pᵢ` (weighted Fisher, scale-corrected per `poolr::fisher`).
- Stouffer: `Σ wᵢ Φ⁻¹(1−pᵢ) / √(Σ wᵢ²)` → Normal.
- Tippett-min: `k·min(p)`, Tippett-max: `k·(1−max(p))`, both Beta-derived.
- Wilkinson: order statistic `p_(k)`。

### 4.4 Group D — Test → Confidence Set

| Kind                          | Spec                                                                                              | Output                                                                 |
|-------------------------------|---------------------------------------------------------------------------------------------------|------------------------------------------------------------------------|
| `hypothesize.invert_test`     | `kind: "wald"\|"z"`, `estimate: number`, `se: number` (or `sigma`,`n`), `grid: [f64]`, `alpha: f64` | one row: `lower`, `upper`, `n_grid_in_set`, `alpha`, `level`         |

This wraps `hypothesize::invert_test`. The closed-form Wald/z CI is exposed
as a column on every wald/z output (`conf_low`, `conf_high`), so the
inversion node is only needed when the test function is non-standard or the
user wants an explicit grid search (e.g. for teaching, or for asymmetric
likelihood-root CIs).

---

## 5. The Closure Property in Practice

Concrete DAGs demonstrating composability. All three are valid
data-engine DAGs today once §4 nodes exist.

### 5.1 Per-stratum test → Fisher combination

A meta-analysis of three cohort GWAS hits:

```
file_to_dataframe(study1.csv) ─┐
file_to_dataframe(study2.csv) ─┼─ hypothesize.t_test(x="effect", mu=0) ─┐
file_to_dataframe(study3.csv) ─┘                                         ├─ hypothesize.combine_pvalues(method="fisher")
                                                                  └─ (single row, combined p)
```

### 5.2 Bioequivalence via intersection-union (TOST)

```
file_to_dataframe(pk.csv)
   └─ hypothesize.t_test(x="auc_t", y="auc_r", paired=true, alternative="less",  mu=+ln1.25)
   └─ hypothesize.t_test(x="auc_t", y="auc_r", paired=true, alternative="greater", mu=−ln1.25)
        └──┬───┘
           └─ hypothesize.boolean_test(op="intersection")   # rejects ⇒ bioequivalent
```

### 5.3 GWAS hit scan with FDR control

```
file_to_dataframe(sumstats.tsv)
   └─ sql_node("SELECT chr, pos, beta, se FROM port_0")
       └─ hypothesize.wald_test(estimate_from="beta", se_from="se")   # one row per SNP
           └─ hypothesize.adjust_pvalues(method="bh")
               └─ sql_node("SELECT * FROM port_0 WHERE p_adj < 0.05")
```

For Group B nodes like `wald_test` we also accept a **column-mode** spec
(`estimate_column`, `se_column`) so they vectorise over input rows, making
§5.3 a single node rather than a row-map.

---

## 6. Cross-Validation Plan

Following the convention used by `epi`, `ldsc`, `susie`, etc. (see
[`sta_epi_nodes.md`](sta_epi_nodes.md) §9):

| Crate test file           | Scope                                            | R reference invoked                                  |
|---------------------------|--------------------------------------------------|------------------------------------------------------|
| `xval_stats.rs`           | every Group A primitive                          | `t.test`, `wilcox.test`, `var.test`, `bartlett.test`, `kruskal.test`, `friedman.test`, `cor.test`, `ks.test`, `shapiro.test`, `prop.test`, `fisher.test`, `chisq.test`, `oneway.test`, `aov` |
| `xval_hypothesize.rs`     | trinity + combinators + adjust + invert          | `hypothesize::wald_test`, `lrt`, `z_test`, `score_test`, `fisher_combine`, `intersection_test`, `union_test`, `complement_test`, `adjust_pval`, `invert_test` |
| `xval_padjust.rs`         | all 7 `p.adjust` methods on tricky vectors       | `stats::p.adjust`                                    |
| `xval_coin.rs` (optional) | exact Wilcoxon / KW / Fisher confidence intervals | `coin::wilcox_test`, `exactRankTests`                |

Golden vectors live in `stat_crates/hypothesize/fixtures/` as TSVs
side-by-side with the corresponding R driver scripts (`driver.R`) so anyone
can regenerate via `Rscript driver.R`. Data archives follow the
[test-data archive convention](MEMORY.md) for any non-trivial fixture.

### 6.1 Numerical choices to match R

- Continuity corrections default to **on** where R defaults to on
  (`wilcox.test`, `prop.test`, `ks.test` exact). Override via spec.
- Shapiro–Wilk uses **Royston's AS R94** algorithm (R's `swilk`), including
  the n>5000 rejection and the n=3 special-case.
- Two-sample t-test defaults to **Welch** (`var_equal=false`), matching R.
- `chisq_gof` defaults to `rescale_p=true`, matching R `chisq.test(x, p)`.
- `ks.test` asymptotic vs exact selection follows R 4.x rules
  (exact when `n·m ≤ 10000` for two-sample).

---

## 7. Registration

### 7.1 Registry wiring

Each Group gets a `register_hypothesize_nodes(registry: &mut NodeRegistry)`
helper called from `NodeRegistry::new()` in
`crates/data-engine/src/node_registry/registry.rs`. Factories live under
`nodes::hypothesize::*` and are re-exported in `nodes/mod.rs`.

---

## 8. Error Handling

Following the per-node error enum pattern used by `chi_square.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum HypoTestError {
    #[error("column '{0}' not found or wrong type")]
    Column(String),
    #[error("test failed: {0}")]
    Test(String),
    #[error("invalid spec: {0}")]
    Spec(String),
    #[error("collect failed: {0}")]
    Collect(String),
}
```

Each node converts into `DagError::NodeError { node_type, msg }` so the
agent-facing failure surface is uniform. Spec validation rejections are
surfaced via the existing `SpecRejection` path (the registry's
schema-driven normaliser handles object-wrapped-array repair for free).

### Common spec-rejection rules (enforced in `build()`)

- `t_test`: `x_column` required; `y_column` xor `group_column` for two-sample.
- `wilcoxon` paired: `x_column` and `y_column` must have equal length after
  null-filtering; same for `t_test` paired.
- `prop_test`: counts ≤ n; counts and n same length.
- `shapiro_test`: 3 ≤ n ≤ 5000.
- `fisher_exact`: input table is exactly 2×2.
- `wald_test`: exactly one of `se` / `vcov`; multivariate `estimate` and
  `vcov` have matching dimension; `vcov` symmetric positive-definite (we
  attempt Cholesky and reject on failure, mirroring R's `solve()` error).
- `lrt`: `alt_loglik ≥ null_loglik` (warn, don't fail, matching R's
  `warning()`); `dof > 0`.
- `combine_pvalues`: all `p_value`s in `(0, 1]`; weights non-negative and
  sum > 0.

---

## 9. Phasing

| Phase | Scope                                                                 | Outcome                                                  |
|-------|----------------------------------------------------------------------|----------------------------------------------------------|
| **1** | `hypothesize` crate: trinity + combinators + adjust + invert (ports) | `xval_hypothesize.rs` passes (matches `hypothesize::`)   |
| **2** | `hypothesize` crate: t/z/prop/var/cor/ks/shapiro/chisq_gof/fisher     | `xval_stats.rs` passes (matches `stats::`)               |
| **3** | `hypothesize` crate: ranks (wilcoxon/MW/KW/friedman) + Bartlett/Levene/Fligner + ANOVA + AD | `xval_stats.rs` extended                  |
| **4** | DAG nodes: Group A primitives                                        | every primitive emits the standard test-row schema       |
| **5** | DAG nodes: Group B (trinity), Group C (combinators), Group D (invert)| compositional DAGs §5 runnable end-to-end                |
| **6** | Documentation (`hypothesize-design_zh.md`)                            | bilingual design doc                                      |

Each phase is independently shippable: Phase 1 alone gives the agent a
fully composable likelihood-trinity + meta-analysis toolkit.

---

## 10. Future Work (out of scope here)

- **Permutation / bootstrap engine** — a `hypothesize.permutation_test`
  node wrapping a generic `Fn(&Data) -> f64` test statistic with
  stratified resampling. Pairs naturally with the existing exact tests.
- **Bayesian testing** — Bayes-factor nodes (`BF01`, `BF10`) consuming the
  same `HypothesisTest`-shaped inputs (likely as a sibling `bayes_tests`
  crate to avoid mixing paradigms).
- **Multiple-comparison procedures beyond `p.adjust`** —
  closure-derived adjusted p-values (e.g. weighted BH, adaptive BH),
  single-step / step-down maxT/minP (Westfall–Young permutation null).
- **Heterogeneity-combined tests** — DerSimonian–Laird random-effects
  meta-analysis (extends `combine_pvalues` with between-study variance).
- **Sequential / alpha-spending** — group-sequential boundaries (O'Brien–
  Fleming, Pocock) for prospective designs.
- **Equivalence / non-inferiority** as first-class nodes (currently
  expressible via §5.2's TOST pattern but could be packaged).

---

## 11. Why This Shape

Three structural decisions deserve explicit justification:

1. **One crate, two layers** (typed core + DAG nodes). Mirrors `statkit` /
   `epi` / `lava` / `susie`. Keeps the crate usable outside the DAG
   (e.g. by other crates via `faer` directly), and keeps nodes thin — every
   node is "read columns, call one crate function, write a test row".

2. **Standard test-row schema as the DAG closure witness**. R's closure
   property (combining tests yields tests) only works because every test
   object has the same accessor API. In a DAG the analogue is "every test
   node emits the same schema", and combinators consume that schema —
   giving us `test → test` composition for free, with arbitrary nesting.

3. **Group B accepts both literal spec values and column inputs**. Pure
   literal specs (R `hypothesize::wald_test(estimate=…, se=…)`) are
   natural when the test is *the* output; column mode
   (`estimate_column`, `se_column`) is needed for vectorised pipelines like
   §5.3 where one node produces thousands of test rows. Supporting both
   costs little and avoids forcing a `map` node into every vectorised
   pipeline.
