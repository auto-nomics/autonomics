# SuSiE-RSS

`susie_rss_container` performs Bayesian fine-mapping from GWAS summary
statistics with the official `susieR` R package. The registered node runs
`susieR` 0.16.6 in an ephemeral k3s Job and mounts the cataloged
`mixer.g1000_eur` signed-LD panel.

## Runtime And Reference

The `mixer.g1000_eur` package pins the GRCh37 EUR panel, per-chromosome PLINK
BIM files, signed Pearson-r LD matrices, and the `libbgmg.so` checksum. The
container queries signed LD pairs through the gsa-MiXeR engine and passes the
correlation matrix directly to `susieR::susie_rss()`. LD signs are never
reconstructed from GWAS z-score signs.

Alignment uses the full per-chromosome BIM. The package also contains
`snps/g1000_eur_chrN.snps` extraction lists of roughly 254 variants per
chromosome; those lists are leftovers from mixer and are not a constraint for
this node.

## Input Contract

The input is one tab-separated File with these columns:

| Column | Required | Meaning |
| --- | --- | --- |
| `snp` | yes | Variant key accepted by the mixer BIM |
| `chrom` | yes | Chromosome; every row must use the same chromosome |
| `z` | yes | GWAS z-score; missing values are rejected |
| `n` | optional | Per-locus sample size; `spec.n` overrides it |
| `a1` | optional | First allele for LD alignment |
| `a2` | optional | Second allele for LD alignment |

Use `sink_file` with `format: "tsv"`:

```json
{
  "path": "/locus.tsv",
  "format": "tsv",
  "mode": "overwrite"
}
```

The container reads with `read.delim` semantics. A comma-separated file is not
sniffed: its first line becomes one column name and the run fails with
`input is missing required columns: snp, chrom, z`, which does not identify the
real delimiter problem.

Keep `snp` unique upstream, for example with `ROW_NUMBER()` over a partition of
`snp`. SuSiE-RSS is a single-locus model; for large regions, split the input
into loci of at most a few thousand variants. `r2_min > 0` can reduce the
number of LD pairs used to construct the dense matrix.

## SNP Key Format

The default panel uses GRCh37 position-type BIM IDs:

```text
chr:pos:allele1:allele2
21:36119111:G:T
```

The `snp` column must contain these literal IDs. rsIDs cannot align and produce
an error such as `N input SNP(s) did not align to the signed-LD reference`.

When `a1` and `a2` are omitted, they are inferred from the last two colon
separated fields of the ID. Explicit allele columns may use either orientation;
the signed-LD query adjusts the correlation sign for a flipped allele pair.

## Outputs

Each run publishes immutable artifacts below
`/artifacts/susie_rss_container/<job-id>/` with content fingerprints. Ports are
positional and currently unnamed:

| Port | Artifact | Contents |
| --- | --- | --- |
| 0 | `susie_rss.tsv` | `snp`, `pip`, `cs`, `alpha`, `mu`, `mu2`, `lbf` |
| 1 | `susie_rss.RDS` | Official `susieR` fit object for `readRDS()` |
| 2 | `susie_rss.log` | Variant count, iterations, convergence, and credible sets |

In the TSV, `cs=0` means the variant is not in a reported credible set.
`cs=k` for `k>=1` means it belongs to credible set `k`, corresponding to single
effect `L_k`. `alpha` is the inclusion probability of the best single effect
for that variant. Posterior moment output should satisfy `mu * mu <= mu2`.

## Failure Behavior And Parameters

Missing columns, missing z-values, multiple chromosomes, unknown SNP keys, and
a multi-variant locus with no overlapping LD pairs all fail the run. Duplicate
SNP keys are not useful input even when the underlying engine happens to accept
them; deduplicate upstream. The default runtime limit is 1800 seconds.

| Parameter | Default | Meaning |
| --- | ---: | --- |
| `l` | `10` | Maximum number of non-zero effects |
| `estimate_prior_method` | `"optim"` | One of `optim`, `EM`, or `simple` |
| `estimate_residual_variance` | `false` | Estimate residual variance each IBSS iteration |
| `estimate_prior_variance` | `true` | Estimate prior variance per effect |
| `coverage` | `0.95` | Credible-set target coverage |
| `min_abs_corr` | `0.5` | Minimum absolute correlation for credible-set purity |
| `scaled_prior_variance` | `0.2` | Initial scaled prior variance |
| `z_method` | `"wald"` | One of `wald` or `score` |
| `r2_min` | `0.0` | Minimum `r*r` for signed LD pairs |
| `n` | `null` | Sample-size override; otherwise input column `n` |
| `check_null_threshold` | `0.0` | `susie_rss` check-null threshold |
| `max_iter` | `100` | Maximum IBSS iterations |
| `artifact_prefix` | `/artifacts/susie_rss_container` | Immutable output prefix |
| `timeout_secs` | `1800` | Container timeout |

## Numerical Regression Baselines

For the same 223-SNP synthetic locus and identical parameters, the previous
native Rust implementation and the official container agreed to 8-9 significant
digits:

| SNP | Metric | Native Rust | Container |
| --- | --- | ---: | ---: |
| `21:46783784:G:T` | `pip` / `lbf` | `1.0` / `66.225242` | `1` / `66.225242` |
| `21:46777491:C:T` | `pip` / `mu` | `0.99999899` / `-0.0133920760` | `0.99999899` / `-0.0133920761` |
| `21:45513808:C:T` | `pip` | `0.16537501` | `0.16537501` |

A real AF GWAS chr21:36.12Mb locus keyed by `rs2834618`, rather than by its
positional ID, is a useful negative alignment case. After conversion to
`21:36119111:G:T` with `z=-8.43`, the run produced a PIP near 1.0, a 95%
credible set, `lbf=34.2`, and convergence in 14 iterations.

## rsID Conversion SQL

For rsID sumstats, join on chromosome, GRCh37 position, and the allele pair.
The following pattern also handles the panel's headerless BIM when
`source_file` consumes its first physical row as column names:

```sql
WITH bim AS (
  SELECT "21:9411410:C:T" AS vid,
         CAST("21" AS BIGINT) AS rchrom,
         CAST("9411410" AS BIGINT) AS rbp,
         "T" AS ral1,
         "C" AS ral2
  FROM port_1
), joined AS (
  SELECT bim.vid AS snp,
         af."CHR" AS chrom,
         af."Z" AS z,
         CAST(af."N" AS DOUBLE) AS n,
         af."A1" AS a1,
         af."A2" AS a2,
         ROW_NUMBER() OVER (
           PARTITION BY bim.vid
           ORDER BY ABS(af."Z") DESC
         ) AS rn
  FROM port_0 af
  JOIN bim
    ON af."CHR" = bim.rchrom
   AND af."BP" = bim.rbp
   AND ((af."A1" = bim.ral1 AND af."A2" = bim.ral2)
     OR (af."A1" = bim.ral2 AND af."A2" = bim.ral1))
  WHERE af."BP" BETWEEN 36070000 AND 36170000
)
SELECT snp, chrom, z, n, a1, a2
FROM joined
WHERE rn = 1;
```

If the target locus includes the first BIM row, add that row back explicitly;
the quoted names in `bim` are the values consumed as its header.
