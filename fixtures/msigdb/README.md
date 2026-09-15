# MSigDB GMT fixtures

Gene-set fixtures for the `gmt_import` source node, `enrichment_ora`, and the
`pathway_gsea` container node.

## Provenance

Downloaded from the official MSigDB release site (GSEA-MSigDB, Broad
Institute), release `2025.1.Hs`, gene symbols version:

- <https://data.broadinstitute.org/gsea-msigdb/msigdb/release/2025.1.Hs/h.all.v2025.1.Hs.symbols.gmt>
- <https://data.broadinstitute.org/gsea-msigdb/msigdb/release/2025.1.Hs/c2.cp.v2025.1.Hs.symbols.gmt>

MSigDB content is distributed under CC BY 4.0. Cite Subramanian et al. (2005)
and Liberzon et al. (2011, 2015) when used in publications.

## Files

| File | Sets | Gene annotations | Size | Use |
|---|---:|---:|---:|---|
| `h.all.v2025.1.Hs.symbols.gmt` | 50 | 7,322 | 48,689 B | Hallmark; primary fixture for tests and demos |
| `c2.cp.v2025.1.Hs.symbols.gmt` | 4,023 | 173,410 | 1,626,547 B | Canonical pathways; larger realistic corpus |
| `hallmark_minimal.gmt` | 3 | 30 | 495 B | Derived: first 3 Hallmark sets trimmed to 10 genes each; fast unit tests only |

`hallmark_minimal.gmt` is a derived fixture, not an official MSigDB artifact;
do not use it for graded results.

## Integrity

SHA-256 checksums are recorded in `manifest.json`. Verify before first use:

```sh
sha256sum -c <(python -c "import json; [print(f\"{v['sha256']}  {k}\") for k, v in json.load(open('manifest.json')).items()]")
```

## Format

Standard GMT: one gene set per line, tab-separated — set name, description
(URL), then gene symbols. All three files validated: no empty gene fields, no
sets with fewer than 3 genes.
