# Zotero Reading Mode and structured extraction

The GUI's **Read** button (Alt+R) switches between the original PDF and semantic
reflow. **Aa** opens the appearance panel. Font size, serif/sans font, line
height, word/letter spacing, narrow/normal/full width, and light/sepia/dark themes
are independent of PDF zoom. The reading position and appearance survive
reopening the document. Thumbnails and the page input navigate the source PDF's
pages in both modes; the outline in Reading Mode comes from the SDT catalog.

## Source analysis and port

This is a direct source port of the extraction chain that powers Zotero's
Reading Mode, rather than an imitation based on the existing flattened text.
Pinned upstream implementations are in `tools/zotero-extractor/vendor`:

1. [document-worker getStructure](https://github.com/zotero/document-worker/blob/a8bab24c82a41435f125a17a4865951b74518117/src/pdf/index.js)
   opens the original bytes with Zotero's PDF.js fork. Its custom PDF module
   extracts characters, objects, font/line information and exact PDF geometry.
2. [getFullStructure](https://github.com/zotero/document-worker/blob/a8bab24c82a41435f125a17a4865951b74518117/src/pdf/structure/structure.js)
   runs the original ONNX block clusterer, repair model and classifier, then
   Zotero's structure cleanup. Its SDT retains paragraph boundaries, headings,
   lists, tables, notes, citations, excluded page furniture, outline and page
   ranges. Paragraph part chains preserve text continued across columns/pages.
3. [SDT fulltext projection](https://github.com/zotero/structured-document-text/blob/7963c11faac4595be3bbe86b7574704a22c67bed/src/fulltext.js)
   produces the searchable text with paragraph separators. It is stored without
   the old `split_whitespace().join(" ")` normalization.
4. [Reader semantic renderer](https://github.com/zotero/reader/blob/4bfd82ee848e36e80aaff2a2c732615c89ad1e72/src/dom/sdt/lib/renderer.ts)
   emits paragraphs, headings, lists, tables and stable block/text references.
   The GUI uses this original renderer, including cross-page paragraph merging.
5. [PDFPositionMapper](https://github.com/zotero/reader/blob/4bfd82ee848e36e80aaff2a2c732615c89ad1e72/src/common/sdt/pdf-position-mapper.ts)
   maps semantic character ranges to original PDF character rectangles and back.
   Reading Mode annotations use this source map; reflow screen rectangles are
   never persisted as PDF coordinates.

The original modules/models are unmodified. The added code supplies the Node
host, Rust process/storage adapter, GUI controls, source-crop rendering and
existing bibliography API integration. Version pins, notices and regeneration
instructions are documented in [the extractor package](../tools/zotero-extractor/README.md).

## Storage and migration

New PDF uploads and re-extraction try Zotero SDT first, then MinerU.
The old local PDF text extractor and Tesseract OCR have been removed.
If both PDF backends fail or yield no text, extraction is marked failed and the
original file remains available for retry. HTML/TXT keep their built-in extraction. The complete SDT is stored separately from the searchable
text, under the attachment key:

```text
vfs:///literature/{article}/attachments/{key}/extracted/fulltext.sdt.json
vfs:///literature/{article}/attachments/{key}/extracted/fulltext.txt
```

The cache envelope contains the original VFS path/hash and processor version.
An attachment change or processor-version mismatch invalidates it. Deleting,
replacing or re-extracting an attachment removes the old semantic sidecar.
A successful fallback without SDT also removes any stale source map.

The first Reading Mode open upgrades an older PDF on demand. It does not clear
the old extracted text before conversion, so a failed conversion leaves the PDF
and existing text usable. A successful conversion stores both SDT and the
paragraph-preserving fulltext projection. Batch re-extraction of the library is
not triggered automatically.

API:

- `GET /api/v1/bib/articles/{id}/fulltext/structured`: read a valid cache, or 404.
- `POST /api/v1/bib/articles/{id}/fulltext/structured`: reuse a valid cache or
  generate SDT and upgrade the text projection.
- The same GET/POST routes exist under
  `/articles/{id}/attachments/{key}/fulltext/structured` for an explicit attachment.

Generation uses the normal extraction concurrency limit and coalesces repeated
requests. Stale results are rejected if the original file changes during work.
The desktop bridge allows conversion requests a longer deadline than ordinary
API calls. Closing a view during generation cannot create an orphan reading
view. Reopening reuses the attachment cache.

## Figures, annotations and limits

Figures, formulas and non-semantic table representations are lazy crops of the
original PDF, using SDT's source rectangles. Tables with semantic cells render
as HTML tables. Cross references stay inside the reading view; external links
are restricted to HTTP(S)/mail. Existing PDF highlights are mapped onto the
reflowed text and recalculated when layout changes.

Zotero's mapper represents a selection on one page or two adjacent pages. The
current annotation API stores one page per annotation, so a selection spanning
two pages is saved as two page-anchored highlights. Larger selections remain
copyable but cannot be highlighted through this API. Legacy highlights saved
only in display coordinates are still displayed in the PDF; they are not
invented as semantic Reading Mode positions.

This port does not add a local OCR engine. Image-only PDFs can use the MinerU
fallback; missing structured text is reported rather than silently presenting
flattened OCR output as Zotero SDT. Layout/model edge cases retain
Zotero's own per-page degradation behavior. The full Zotero desktop note editor,
EPUB/snapshot readers, read-aloud UI and split views are outside this PDF port.

## Verification

- Full SDT equality against five unchanged upstream semantic golden fixtures:
  reference links/lists, cross-page notes/paragraphs, printed contents, real
  scientific figures/tables and cross-page column-width variation.
- Vendored source/model checksums; malformed-PDF failure.
- Rust process tests with actual Node/ONNX extraction; paragraph/anchor retention.
- Rust storage/API tests for source/version invalidation and attachment cleanup.
- Chromium tests using real extraction output: semantic layout, appearance,
  exact source-map highlight coordinates, mode toggles, location restoration,
  conversion failure and retry.
- Existing PDF text-alignment tests across DPR 1/1.5/2 and all page rotations.
