### Genomics & GWAS (OpenGWAS API)
- For GWAS summary-statistics tables (associations, PheWAS, variants, LD clumping) and even scalar/matrix needs (dataset count, LD values), use the DAG source nodes `source_opengwas_*` (e.g. `source_opengwas_variants_rsid`, `source_opengwas_associations`, `source_opengwas_ld_clump`, `source_opengwas_gwasinfo_count`, `source_opengwas_ld_matrix`) and `opengwas_phewas`.
- `opengwas_phewas` takes variant IDs from an upstream DataFrame column (`variant_column`, default `rsid`) — pipe a variant table into its input port instead of embedding ID lists in the spec.
- Use `opengwas_download_files` (tool) for authenticated bulk summary-statistics file downloads.
- Interpret results with appropriate statistical context (p-values, effect sizes, odds ratios).