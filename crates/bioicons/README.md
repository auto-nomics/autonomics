# BioIcons adapter

Reads the public [BioIcons catalogue](https://bioicons.com/) and prepares standalone SVG assets plus PNG vision previews. This supplements the NIH BioArt PNG adapter. It does not treat the site's code licence as the licence of every icon: each result retains its original author, licence code/link, source page, SVG URL and catalogue SHA-256. See the upstream [attribution guidance](https://github.com/duerrsimon/bioicons#credits).

`BioIconsClient::search(query, start, limit)` searches names, categories and authors. A client caches one catalogue snapshot; IDs derive from canonical catalogue paths, and arbitrary user URLs are not accepted for downloads. `vector(id)` downloads the matching SVG, records its original SHA-256, normalizes it using usvg/resvg, and returns a second SHA-256, SVG data URL, PNG preview, dimensions, observed fill/stroke/gradient colours and normalization notes. Unknown licence codes remain unknown rather than being inferred as CC0.

The renderer has no external image resolver or system font lookup. Active elements, embedded rasters, external references, escaped CSS references, unsupported SVG elements and unsupported glyphs are rejected. Text is outlined using bundled Liberation Sans; its SIL OFL and notices are in `fonts/OFL.txt`. This conversion can change typography; inspect the preview and retain the normalization notes. Input, node/event counts, nesting, viewport and normalized output are bounded. Some otherwise valid SVGs are unsupported; errors are explicit rather than silently removing raster images or scientific text.

```rust,no_run
# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = bioicons::BioIconsClient::new()?;
let page = client.search("receptor", 0, 12).await?;
let vector = client.vector(&page.assets[0].id).await?;
assert!(vector.vector.data_url.starts_with("data:image/svg+xml;base64,"));
# Ok(()) }
```

`cargo test -p bioicons` uses bounded HTTP fixtures and real SVG parsing/rendering. `cargo run -p bioicons --example inspect -- receptor /tmp/new-bioicons-prefix` optionally queries the real catalogue and writes fresh `.svg` and `.png` files (existing paths are not overwritten). A live check on 2026-11-10 preserved the two P labels in the `receptor` icon by Jan-Jezierski, with the source-provided `cc-0` metadata.

The GUI's vector panel uses `/api/v1/visualization/bioicons/search` and `/bioicons/{id}/vector`. Newly inserted assets first seek visible whitespace and preserve the positions of existing objects; a full viewport falls back to a free area beside the scene and pans to the new asset. SVG remains an independent scalable image in the canvas; its paths are not separate editor objects yet. Exact paint-colour substitutions are available through `vector::recolor` and the shared `visualization-assets` review flow; replacements are simultaneous and preserve glyph/path geometry and opacity. The `bioicons` agent tool searches, returns PNG vision previews, and proposes insertion/replacement with immutable revision checks. Both mutations await review. `visualization read_image` rasterizes saved SVGs for vision without replacing their stored vector source.

Opt-in browser validation (live public HTTP requests, no model calls): build the native example, then run `VISUALIZATION_BIOICONS_LIVE=1 node tests/canvas-native.mjs` from `apps/gui`. It checks real catalogue search, preview, insertion, metadata, save/reload and that figure SVG export retains an SVG image. The normal native suite remains independent of the public catalogue.

The normalized SVG root includes explicit dimensions and viewBox in the editor-compatible serialization. Repeated normalization retains identical vector bytes, hash and preview; browser tests also check that Excalidraw loading does not rewrite those bytes.
