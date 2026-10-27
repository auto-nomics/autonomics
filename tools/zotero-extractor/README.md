# Zotero structured PDF extraction

The PDF extraction order is **Zotero SDT → MinerU**.
Reading Mode requires SDT and reports conversion failures instead of showing
flattened fallback text as structured content.

## Setup

Use Node.js 22 or newer:

```sh
npm ci --prefix tools/zotero-extractor
```

The ONNX models, PDF.js sources, CMaps, font files and decoders are vendored.
Conversion runs locally on the CPU and does not download models or send PDF
content to Zotero. The fallback chain can still use MinerU when configured.

Run the adapter directly:

```sh
node tools/zotero-extractor/extract.mjs < paper.pdf > result.json
```

Input is PDF bytes on stdin. Stdout is one JSON response with `text`,
`extractor: "zotero-sdt"`, and the complete `structure`. Paragraph separators are
preserved by Zotero's own `getFulltextFromStructuredText`. Diagnostics and progress
are written to stderr. No uploaded filename becomes a shell command.

Rust enforces a 180-second subprocess timeout, cancels the child if the request
is dropped, bounds response/diagnostic sizes, and drains both pipes while
writing the input. Each Node process runs ONNX single-threaded; process-wide
bibliography extraction permits bound CPU concurrency.

## Packaging

Release builds run `node tools/zotero-extractor/package-runtime.mjs <output>`
on the target OS/architecture. The output contains the pinned sources/models,
production dependencies, a copy of Node and its license. Ship it next to the
server as `zotero-extractor/`; the release archive and installer already do this.
The packaged Node is used automatically, so release installations do not need a
separate system Node installation. `@napi-rs/canvas` includes a native component,
which is why packaging must run on the target platform.

`ZOTERO_EXTRACTOR_PATH` can point to `extract.mjs` elsewhere. `ZOTERO_NODE` can
point to a Node executable. Source-tree builds discover the checked-in adapter
automatically. Source inputs are pinned in [UPSTREAM.json](UPSTREAM.json); no
floating Git/network dependency is used at runtime.

## Development and verification

```sh
npm --prefix tools/zotero-extractor test
node tools/zotero-extractor/build-frontend.mjs
npm --prefix apps/gui run test:reading
npm --prefix apps/gui run test:pdf
cargo test -p bib-base --lib zotero_extract::tests -- --ignored
cargo test -p bib-base --lib structured_tests
cargo test -p api-server --test api bib_structured_fulltext
cargo test -p api-server --test api bib_zotero_migrates -- --ignored
```

The JavaScript tests compare five synthetic/CC-BY scientific PDF fixtures against the **entire SDT structure** to Zotero's semantic
golden fixtures, normalizing only creation date and processor version as in
upstream's test helper. Browser tests feed real extraction results through the
reader, check semantic blocks, appearance changes, PDF annotation mapping,
position restoration and failed-conversion recovery.

Rebuild the generated GUI module after changing the vendored reader version.
Review the processor/schema versions in Rust's cache validator when updating
upstream. See [THIRD_PARTY.md](THIRD_PARTY.md) for provenance and notices.
