# SuSiE-RSS

`susie_rss` performs Bayesian fine-mapping from GWAS summary statistics with
`snp`, `chrom`, `z`, and optionally `n`, `a1`, and `a2` columns. The fitting
engine is the pure-Rust SuSiE-RSS port, cross-validated against susieR golden
outputs.

## Signed LD Reference

The node resolves a semantic `reference` ID, defaulting to `g1000_eur`. It
reuses the deployed gsa-MiXeR bundle at:

```text
/mnt/data/mixer/resources/g1000_eur/bundle.json
```

The bundle pins the GRCh37 EUR panel, matching per-chromosome BIM files, signed
Pearson-r LD matrices, and the `libbgmg.so` checksum. At execution time the node
queries signed LD pairs for the input locus through the gsa-MiXeR engine and
passes the correlation matrix directly to SuSiE. It never reconstructs LD signs
from GWAS z-score signs.

Input SNP IDs must match the reference BIM IDs. The deployed 1000 Genomes panel
uses IDs such as `21:9411410:C:T`. A custom signed-LD bundle can be selected by
its semantic `reference` ID; raw LD paths are not exposed in DAG specs.
When `a1`/`a2` are omitted, alleles are inferred from a `chr:pos:a1:a2` ID;
non-coordinate SNP IDs require explicit allele columns. Duplicate IDs and
partially aligned loci are rejected rather than silently represented as diagonal
entries.

The `r2_min` option filters sparse signed LD pairs by `r * r` before matrix
construction. For a locus with multiple variants, an input set with no LD-pair
overlap is rejected rather than silently treated as a diagonal matrix.
