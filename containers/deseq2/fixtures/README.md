# DESeq2 pasilla fixture

This fixture contains the official Bioconductor DESeq2/pasilla vignette inputs
for a two-condition RNA-seq differential-expression model:

- `pasilla_gene_counts.tsv`: 14,599 genes by 7 samples of raw integer counts.
- `pasilla_sample_metadata.tsv`: condition and library-type metadata in the
  same sample order as the count-matrix columns.

The count matrix is copied verbatim from Bioconductor `pasilla` 1.40.0. The
metadata is derived from the package's `pasilla_sample_annotation.csv` by
retaining `condition` and `type`, changing the sample IDs from `<sample>fb` to
`<sample>`, and ordering rows by the count-matrix columns. This is the same
alignment performed in the official DESeq2 vignette.

The count matrix and annotation used to derive the metadata are identical to
the copies distributed with the planned runtime's own DESeq2 1.50.2 source
archive; the fixture is checked in separately so tests do not depend on package
installation order or cached library paths.

Provenance:

- Source package: <https://bioconductor.org/packages/release/data/experiment/html/pasilla.html>
- Source version: 1.40.0
- Source archive SHA-256:
  `75122d45eb2c415d3e94d1b9bc9e4f6978c4504ef7e70f6bfeccb20547cb3db5`
- License: LGPL
- Experiment: Brooks et al., Genome Research 2011, PMID 20921232
- Accession range: GSM461176-GSM461181

`pasilla_baseline.json` records the lightweight result baseline from two
repeated runs with the pinned local DESeq2 image: output SHA-256 values,
independent-filtering counts, direction counts, size factors, and the estimates
for two genes. The fixture and image test scripts moved with the DESeq2
image build tree to the deseq2 manifest plugin
(`/mnt/projects/node-plugins/deseq2/test_deseq2_fixture.sh` and
`test_deseq2.sh` by default; the plugin directory is the single source of
truth for the tool). `test_deseq2_fixture.sh` checks the input
checksums and structural contract; `test_deseq2.sh` checks
the full numerical baseline and independently recomputes normalization.
