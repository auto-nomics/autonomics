# KEGG Rust SDK

An async client for the KEGG REST API at `https://rest.kegg.jp`. It covers
`info`, `list`, `find`, `get`, `conv`, `link`, `ddi`, KGML, images, and BRITE
JSON, while enforcing KEGG's documented limit of three requests per second.

The API is available for academic use. Non-academic use requires a KEGG
license.

## Integration surfaces

- `KeggClient` and `parser` provide structured source data for future DAG
  nodes.
- `convert::Table` provides deterministic columns/rows for promotion to
  Arrow or DataFusion without forcing those dependencies into the SDK.
- `kegg_registrations` installs bounded, agent-friendly previews for info,
  search, entry, link, conversion, and DDI queries.

For enrichment workflows, fetch an organism mapping once (for example
`client.link("pathway", "hsa")`), convert it into the generic table, and
cache it by KEGG release rather than querying per gene.
