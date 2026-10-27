# Selected highlights and annotation cards

The PDF reader and Reading Mode share one annotation selection controller.
Click highlighted text to select the annotation; a blue dashed bounding box
surrounds the highlighted lines, with a compact card containing page number,
comment, tags and an options menu. Clicking outside or pressing Escape closes
it. Dragging over highlighted text still makes an ordinary native text selection.
The annotation sidebar opens the same card and reflects the selected item.

The interaction follows these pinned Zotero reader sources:

- [annotation-popup.js](https://github.com/zotero/reader/blob/4bfd82ee848e36e80aaff2a2c732615c89ad1e72/src/common/components/view-popup/annotation-popup.js):
  selected annotation identity, comment/tags controls and disabled quote preview.
- [PopupPreview](https://github.com/zotero/reader/blob/4bfd82ee848e36e80aaff2a2c732615c89ad1e72/src/common/components/common/preview.js):
  page/icon/options header, separate comment editor and tags footer.
- [ViewPopup](https://github.com/zotero/reader/blob/4bfd82ee848e36e80aaff2a2c732615c89ad1e72/src/common/components/view-popup/common/view-popup.js):
  place the card near its anchor and keep it inside the viewport.
- [PDF selected outlines](https://github.com/zotero/reader/blob/4bfd82ee848e36e80aaff2a2c732615c89ad1e72/src/pdf/page.js):
  a single bounding shape around all selected highlight rectangles; blue
  `#6d95e0`, stroke width 2, dashed outline.

`apps/gui/src/annotation-inspector.ts` adapts these behaviors to the existing
DOM/PDF renderer and bibliography API. It does not import the complete Zotero
React application. Painted highlight layers stay transparent to pointer events;
the controller hit-tests their actual rectangles, distinguishing a click from a
text-selection drag. The same geometry keeps the selection and card positioned
after scrolling, zooming, reflow and asynchronous page rendering.

## Card component

The card uses the official shadcn `base-nova` Hover Card composition in
`apps/gui/src/components/ui/hover-card.tsx`, backed by Base UI's
[Preview Card](https://base-ui.com/react/components/preview-card).
`annotation-hover-card.tsx` anchors it to the selected highlight's live geometry.
Base UI handles placement, viewport collisions, portal rendering and transitions.
The controlled card stays pinned after clicking a highlight, including while
moving the pointer away or editing. Outside click and Escape dismiss it.
The editor DOM remains mounted during repositioning, preserving focus and drafts;
closing also commits any tags still in the input. Comment/tag/color persistence,
retry, copy, delete and the selected outline retain their existing behavior.

## Persistence

`PATCH /api/v1/bib/annotations/{id}` accepts any nonempty subset of:

```json
{"comment":"Interpretation of the highlighted text", "tags":["brain","methods"], "color":"#ffd400"}
```

Comment edits are debounced; blur/closing the card flushes pending changes.
Requests are serialized so older responses cannot overwrite newer edits.
Failed edits retain their draft and expose Retry. Tags are added with Enter or
commas and removed individually. Options provide color changes, copy highlighted
text and explicit deletion. Source quote, page, rectangles and attachment identity
are not patchable through this metadata endpoint.

Compatibility rows gain `comment` and `tags` columns through an idempotent
migration. A single storage transaction updates those fields plus Zotero's
`itemAnnotations.comment/commentNormalized/color`, `itemTags`, item modified
version and the search index. Clearing comments/tags does not alter the quote.

Tests cover native and reflow highlight clicks, multiline bounds, native drag
selection, comments/tags/colors, persistence after reopening, failed-save retry,
explicit deletion, old-schema migration, canonical Zotero metadata and unchanged
source geometry. Existing PDF selection/rotation tests remain in place.
