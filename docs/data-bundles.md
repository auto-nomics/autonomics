# Runtime Bundle Registry

Nodes no longer embed deployment paths. Factories declare stable bundle IDs,
and the runtime resolves those IDs through `state_dir/data_bundles.toml`.
`vpath` is always a runtime VFS virtual path, never a host path.

The data engine also installs a built-in registry for factory-declared global
reference bundles. Consequently registry-backed tests and embedded engines can
build nodes such as `sldsc`, `lcv`, and MRlap without duplicating global entries. A
bundle with the same `ident` in `data_bundles.toml` always overrides the
built-in entry.

`ldsc_h2_container` intentionally has no built-in panel fallback. It requires
immutable catalog-backed `DataBundle` records because PanelCache validates each
payload against the catalog manifest before mounting it.

```toml
[[bundle]]
ident = "ldscore.1000g_eur"
desc = "1000G EUR LD Score panel"
vpath = "/bundles/ldsc/1000g.parquet"
```

## Built-in bundle IDs

- `ldscore.1000g_eur`
- `ldscore.1000g_eur_m`
- `ldscore.baselineLD_v2_2_eur`
- `ldscore.baselineLD_v2_2_eur_m`
- `ldscore.ukbb_eur`
- `ldmatrix.1000g_eur`: vpath may contain `{N}` for chromosome-specific tables
- `mixer.g1000_eur`: MiXeR signed-LD/engine bundle root
- MAGMA population panel IDs such as `g1000_eur`, `g1000_eas`, `g1000_afr`,
  `g1000_amr`, and `g1000_sas`
- `magma.gene_loc`
- `als_cns.cell_markers`: ALS / CNS cell-marker bundle (astrocyte, microglia, oligodendrocyte, neuron)
- `kegg.genes`
- `kegg.pathway_ko`
- `kegg.pathways`
- `kegg.genome_pathways`

Custom `bundle_source` nodes may reference any additional registry ID.

## Unified catalog overlay

When `vfs.toml` contains a `[catalog]` section, entries published by
`autonomics-catalog` are added to this registry automatically. Their stable
alias is `/bundles/<id>`, while their immutable version path is
`/datasets/<id>@sha256-<digest>`. Local `data_bundles.toml` entries still take
precedence, so rollout does not change existing nodes. See
[Unified Data Catalog](data-catalog.md).

For `ldsc_h2_container`, the required current catalog IDs are:

- `ldsc.ref_ld.1000g_eur.basic`
- `ldsc.w_ld.1000g_eur_hm3_no_mhc`

For the GCTA summary-statistics nodes, the required current catalog IDs are:

- `plink.ref.1000g_eur.binary`
- `gcta.gene_list.hg19`

For the HDL-L region and scan containers, the required current catalog ID is:

- `hdl.ref.ukb_eur`
