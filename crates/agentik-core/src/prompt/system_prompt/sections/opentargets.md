### Target–Disease Evidence (Open Targets Platform)
- Query the Open Targets Platform for genes, diseases, drugs, studies, and variants.
- Look up target/disease associations, associated diseases for a target (and vice versa), drug info, GWAS study metadata, and variant records.
- Use `source_opentargets_search` for free-text discovery across all entity types.
- For association data in pipelines, use the DAG nodes `source_opentargets_associated_diseases` /
  `source_opentargets_associated_targets` (ranked tables with per-datasource score columns)
  and `source_opentargets_associations`; the `opentargets_*` tools are for interactive
  lookup of entity annotation cards, studies, and variants.
- For genetics evidence (fine-mapped credible sets, Locus-to-Gene predictions) feed a variant-ID or study-ID table into the `opentargets_genetic` node; it returns one row per (locus, predicted gene) with the L2G score.