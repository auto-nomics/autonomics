# autonomics music-deconvolution image

Pinned MuSiC 1.0.0 runtime for reference-based bulk RNA-seq deconvolution.
The image is layered on the published `autonomics/bulk-rnaseq:1.0.0` image and
adds:

- `SingleCellExperiment` 1.32.0
- `TOAST` 1.24.0
- `MCMCpack` 1.7-1
- `nnls` 1.6
- `MuSiC` 1.0.0 at commit `f21fe67f5670d5e9fca0ad7550abaae3423eb59c`

The MuSiC source archive is downloaded from the GitHub commit API and verified
with SHA-256. No reference dataset is embedded.

## Node contract

`music_deconvolution_container` is a compute node, not a source node.

Required File inputs:

1. gene-by-bulk-sample expression/count matrix
2. gene-by-cell single-cell count matrix
3. cell metadata beginning with `cell_id`

Optional File inputs:

4. `cell_type` / `cell_size` table
5. marker gene table

Outputs:

- `cell_type_proportions.tsv`
- `nnls_proportions.tsv`
- `gene_weights.tsv`
- `diagnostics.tsv`
- `run_report.json`

## Fixture

`fixtures/` contains a deterministic 120-gene reference with four donors, two
cell types, ten cells per donor/type, and six pseudo-bulk samples with known
neuron fractions from 0.70 to 0.20. MuSiC estimates approximately
0.711, 0.613, 0.514, 0.413, 0.312, and 0.209 respectively; every proportion
row sums to one and R-squared is approximately 0.9999.

Regenerate the fixture with:

```bash
python3 containers/music-deconvolution/generate_fixtures.py
```

## Build and publish

```bash
./containers/music-deconvolution/test_smoke.sh
```

Current immutable manifest digest:
`sha256:311e32ba2b0e0ce81715bc72f17342604086faeb7eb4964f706f83dcff53c971`.
