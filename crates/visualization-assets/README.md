# Figure asset operations

Shared API/agent layer over `visualization` storage and `bioicons` SVG processing. The storage crate remains independent of the catalogue and renderer. These operations use saved, project-scoped files and do not access the network.

- `palette(store, project, figure, revision, target)` reads actual SVG paint colours and returns a PNG preview. `target` can be an image ID or its existing semantic anchor. It requires the exact current revision.
- `recolor(...)` creates a reviewable proposal from an exact colour map (`#RRGGBB` to `#RRGGBB`). `recolor_document(...)` exposes the pure document transform for hosts that manage their own review flow.

Replacements apply simultaneously to matching fill, stroke, gradient-stop, flood and lighting colours. Invalid or absent source colours fail before proposal creation. Geometry, glyph paths, opacity and unrelated objects remain unchanged. A new content-addressed file is referenced only by the chosen image instance; other instances and original file records retain their original colours. Immutable figure revisions preserve the old artwork.

The image retains original author/licence/source metadata and arbitrary user notes. Its `modifications` list appends the exact map, previous file ID and SHA-256, resulting SHA-256 and timestamp. The upstream `source_sha256` is preserved; imported SVGs without that field record the existing embedded file hash as their starting source. None of this asserts a licence or scientific relationship not already supplied by the user/source.

API:

- `GET .../figures/{id}/vector-palette?element_id=...&expected_revision=N`
- `POST .../figures/{id}/vector-colors` with `{element_id,expected_revision,colors:{"#ff0000":"#00aa88"}}` (actual values must be six hexadecimal digits).

The GUI's “素材配色” control reads one selected SVG, allows exact colour substitution, and opens the canonical before/after proposal viewer. It checks local edits and revisions while reading and proposing. The `bioicons` tool exposes equivalent `palette` and `recolor` actions, with a vision block for the palette preview. PNG images are not supported by these colour operations. Matching colours within one SVG change together; individual paths are not separately selected.

`cargo test -p visualization-assets` verifies instance isolation, preserved metadata/geometry, immutable history, version conflicts and failure atomicity. The live BioIcons browser case additionally covers palette selection, rejected stale/invalid requests, proposal adoption, reload and vector export.

Normalized SVG uses an editor-compatible root namespace/viewBox and document serialization so file loading does not change content-addressed bytes. Browser validation checks the hash of the inserted and reloaded bytes against the recorded file hash.

配色要求 SVG 内文字已转为路径；BioIcons 导入时已处理该转换。含原始文字的其他导入 SVG 会明确报错，避免配色动作悄悄替换字体或改变字形。
