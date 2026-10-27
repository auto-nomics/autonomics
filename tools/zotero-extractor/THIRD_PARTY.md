# Upstream sources

This directory directly vendors Zotero's implementation. It does not reimplement
paragraph detection with whitespace or font-size heuristics. Exact revisions are
recorded in [UPSTREAM.json](UPSTREAM.json); file hashes are in
[vendor/SHA256SUMS](vendor/SHA256SUMS).

- [zotero/document-worker](https://github.com/zotero/document-worker),
  `src/pdf`, `src/versions.js`, and the Node source-loader scripts.
  Copyright Corporation for Digital Scholarship. AGPL-3.0; see
  [COPYING](vendor/document-worker/COPYING). The classifier/clusterer ONNX models,
  statistics, repair model, and runtime data are retained with the code.
- [zotero/reader](https://github.com/zotero/reader), semantic HTML renderer,
  text-node helpers, PDF/SDT position mapper, and constants. AGPL-3.0; see
  [COPYING](vendor/reader/COPYING).
- [zotero/structured-document-text](https://github.com/zotero/structured-document-text),
  the exact submodule revision used by document-worker and reader. It supplies
  the SDT schema, text maps, source ranges, paragraph-part chains, and fulltext
  projection. The upstream package has no separate license declaration; it is
  retained as the submodule included in Zotero's AGPL distribution, without
  relabeling it as MIT.
- Zotero's [PDF.js fork](https://github.com/zotero/pdf.js), including the custom
  character/object extraction module, font data, CMaps and image decoders.
  Apache-2.0; see [LICENSE](vendor/document-worker/pdf.js/LICENSE) and the
  individual third-party notices in the copied files.

`test/fixtures` contains upstream synthetic PDFs and two CC-BY-4.0 scientific
PDF fixtures, their source generators/attribution files, and unchanged semantic
golden results from document-worker's test suite. The `.source.md` files retain
the scientific articles' authors, DOI, license and fixture modifications. It
does not contain user documents.

Vendored implementation files and model binaries are unmodified. The added
`vendor/document-worker/package.json` supplies a Node ESM package boundary.
`extract.mjs` is the host adapter; `build-frontend.mjs` bundles the original
renderer/mapper for the GUI. Its resolver redirects imports to the shared SDT
submodule and supplies two host sort-index utilities so the complete Zotero PDF
viewer is not needed. Reading Mode uses the original per-character source
mapper, not those optional sorting utilities. The generated GUI module is
identified as AGPL-3.0 and must retain the upstream notices.
