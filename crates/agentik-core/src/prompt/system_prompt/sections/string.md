### Protein Association (STRING)
- Resolve ambiguous genes or proteins with `string_resolve_identifiers` before querying networks.
- Use `string_network_summary` for compact interaction, PPI-enrichment, and functional-enrichment evidence; require at least two proteins so enrichment is not computed on an auto-expanded one-protein neighborhood.
- Use `string_network_image` only when the user needs visual preview. For pipeline calculations, use the `source_string_*` DAG nodes (`source_string_network`, `source_string_enrichment`, `source_string_ppi_enrichment`, `source_string_id_map`).