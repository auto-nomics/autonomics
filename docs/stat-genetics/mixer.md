# MiXeR

MiXeR fits univariate (`fit1`) and bivariate (`fit2`) spike-and-slab causal
mixture models to GWAS summary statistics. Autonomics uses a Rust-native
optimizer and cost function for `univariate_mixer`. The reference bundle's
`libbgmg.so` remains a load-time dependency because it decodes binary `.ld`
files, aligns summary statistics, and computes randprune weights; Python is not
launched.

## Nodes

`univariate_mixer` accepts one summary-statistics table with `rsid`, `A1`, `A2`,
`N`, and `Z` columns. It runs the Rust `fit1` engine and returns one row containing
`pi`, `sig2_beta`, `sig2_zero`, `h2`, causal-variant counts, AIC, BIC, and cost.

`bivariate_mixer` accepts trait1 and trait2 summary statistics plus their fit1
parameters. It runs `mixer.py fit2` and returns shared/specific polygenicity,
genetic correlation, Dice similarity, both h2 estimates, and cost.

## Reference Bundle

Both nodes resolve a semantic `reference` ID; the default is `g1000_eur`. The
bundle lives at:

```text
/data/mixer/resources/g1000_eur/bundle.json
```

It pins the gsa-MiXeR source revision, GRCh37/EUR metadata, paths to the
per-chromosome `.bim`, LD, and tag-SNP templates, and the SHA-256 checksum of
`libbgmg.so`. Engine and panel paths are not exposed through DAG specs.

For `univariate_mixer`, Rust loads `libbgmg.so` directly with `libloading`.
FFI calls are serialized because libbgmg stores calculator contexts in a
process-global manager. After extracting LD, AF, tag indices, z/N, and weights,
the node releases the C++ context and folds tag LD rows into sufficient
statistics for `mixer::fit1`.

The container mounts the resource root at the same host path. `bivariate_mixer`
still uses the managed Python runtime with NumPy, SciPy, pandas, Boost, and
OpenMP libraries; `univariate_mixer` does not launch Python. For compatibility
with existing hosts, resolution also tries
`/mnt/data/mixer/resources`. `MIXER_RESOURCE_ROOT` overrides deployment lookup
and may contain a colon-separated candidate list; `MIXER_PYTHON` overrides the
managed Python executable. Configured roots are tried first, followed by the
built-in roots. The first candidate with a valid, checksum-matched bundle wins.
