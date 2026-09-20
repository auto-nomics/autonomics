# ALS / CNS cell marker bundle (`als_cns.cell_markers`)

Versioned data package covering four CNS cell types for the ALS / neurology
rubric tasks. The marker sets are derived from the CellMarker 2.0 Human
release (`2024-10-11`), filtered to `tissue_class` ∈ {Brain, Spinal cord}
and `marker_source` ∈ {Experiment, Review}.

## Contents

| File | Rows | Columns |
|---|---:|---|
| `markers.tsv` | 1,169 | gene_symbol, set_id, set_name, set_class, evidence, marker_source, tissue_class, tissue_type, pmid, journal, year, species |
| `metadata.tsv` | 4 | set_id, set_name, set_class, description, source_url, snapshot_date, gene_count, marker_source_filter |
| `source_manifest.json` | — | Source xlsx SHA-256 + provenance |

| Set | Genes | Rows |
|---|---:|---:|
| `ALS_CNS_ASTROCYTE` | 83 | 232 |
| `ALS_CNS_MICROGLIA` | 367 | 531 |
| `ALS_CNS_OLIGODENDROCYTE` | 56 | 163 |
| `ALS_CNS_NEURON` | 163 | 243 |

## Regenerate

```bash
curl -fsSL https://raw.githubusercontent.com/WilsonWukz/EasyCellMarker2/master/inst/extdata/Cell_marker_Human.xlsx \
    -o /tmp/Cell_marker_Human.xlsx

python3 fixtures/als_cns_cell_markers/build_markers.py \
    --xlsx /tmp/Cell_marker_Human.xlsx \
    --out fixtures/als_cns_cell_markers/staging \
    --snapshot-date 2024-10-11

cargo run -p data-catalog --bin autonomics-catalog -- build \
    fixtures/als_cns_cell_markers/staging \
    fixtures/als_cns_cell_markers/package-v1 \
    --id als_cns.cell_markers \
    --version v1 \
    --kind cell_marker_set \
    --metadata species=Human \
    --metadata tissue_filter='Brain|Spinal cord' \
    --metadata cell_names='Astrocyte,Microglial cell,Oligodendrocyte,Neuron'

cargo run -p data-catalog --bin autonomics-catalog -- validate \
    fixtures/als_cns_cell_markers/package-v1

cargo run -p data-catalog --bin autonomics-catalog -- publish \
    fixtures/als_cns_cell_markers/package-v1 \
    --config ~/.autonomics/vfs.toml
```

After publishing, the bundle is reachable at:

- `/bundles/als_cns.cell_markers` (stable alias)
- `/datasets/als_cns.cell_markers@sha256-2dde836952984aec320bdb4927d72e43c61a78c4d9792234da0aa938343705da`
  (current immutable digest path).

## Wiring into the ORA pipeline

`markers.tsv` is a long-format table; reshape with `datafusion_sql`:

```sql
SELECT gene_symbol, set_id AS pathway
FROM read_parquet('/bundles/als_cns.cell_markers/markers.tsv')
WHERE set_class = 'Glial'
```

then feed the result into `enrichment_ora` as the annotation table.
`metadata.tsv` supplies `set_id → set_name / set_class`. Use `evidence`
to keep only `canonical` (experiment-only) rows when the rubric expects
the strictest set, or include both `canonical` and `supporting` for
recall-oriented ORA.

## Source / license

Source repository: <https://github.com/WilsonWukz/EasyCellMarker2>
Data file: `inst/extdata/Cell_marker_Human.xlsx`
Snapshot date: 2024-10-11
Source commit: `c870a9ff6a8b48390a2fe6fe8054b88738a821ad`
Source payload SHA-256: `bf52b8cd60df60f17a7c6b8a59d0bbb74ec78612f11cd11990f15e4eedb7d842`
CellMarker 2.0 license: CC BY 4.0

Cite the upstream CellMarker 2.0 paper when using these markers in
publications:

> Zhang et al., *CellMarker 2.0: an updated database of manually curated
> cell markers in human/mouse and web tools based on scRNA-seq data*,
> Nucleic Acids Research, 2023.
