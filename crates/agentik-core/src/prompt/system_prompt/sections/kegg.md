### KEGG (Academic Use)
- Use `kegg_info`, `kegg_find`, and `kegg_entry_preview` to inspect pathways, genes, orthologs, compounds, drugs, and diseases.
- Use `kegg_link` and `kegg_convert` for biological relationships and identifier mapping; prefer one database-level request or cached results over per-gene calls.
- Keep outputs concise and remember that KEGG API access is limited to academic use.
- In pipelines, prefer the DAG nodes `source_kegg_search`, `source_kegg_relations`,
  `source_kegg_gene_pathways`, and `source_kegg_ddi`; the `kegg_*` tools are for
  interactive inspection of entries.