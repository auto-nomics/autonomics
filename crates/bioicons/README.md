# BioIcons adapter

Reads the public [BioIcons catalogue](https://bioicons.com/) and prepares standalone SVG assets plus PNG vision previews. This supplements the NIH BioArt PNG adapter. It does not treat the site's code licence as the licence of every icon: each result retains its original author, licence code/link, source page, SVG URL and catalogue SHA-256. See the upstream [attribution guidance](https://github.com/duerrsimon/bioicons#credits).

`BioIconsClient::search(query, start, limit)` searches names, categories and authors. A client caches one catalogue snapshot; IDs derive from canonical catalogue paths, and arbitrary user URLs are not accepted for downloads. `search_with_author(query, start, limit, Some(author))` restricts results to an exact catalogue author name (case and outer whitespace ignored) before ranking/counting/pagination. Filtered receipts retain `author_filter`, including zero-hit results. Same-author attribution does not establish a consistent visual style or scientific identity. GUI search accepts `author`; the Agent tool accepts it for both `search` and `search_batch`, applying it to every query. Omit it to search all authors. `vector(id)` downloads the matching SVG, records its original SHA-256, normalizes it using usvg/resvg, and returns a second SHA-256, SVG data URL, PNG preview, dimensions, observed fill/stroke/gradient colours and normalization notes. Unknown licence codes remain unknown rather than being inferred as CC0.

Search retains the existing substring match set, then stably ranks exact names and complete name tokens ahead of name prefixes, name substrings and metadata matches. `match_summary` counts all six lexical tiers before pagination. These are lexical signals, not scientific validation: for example, a `RAS` query may match `raster` or `raspberry` without identifying a RAS protein illustration. The GUI displays a partial-match notice when neither an exact name nor a whole name token matches; inspect the preview and attribution before choosing any artwork. Refine the query or report that no suitable icon has been confirmed rather than substituting an unrelated result.

The Agent search tools additionally attach `search_scope` without nesting or replacing existing result fields: it echoes the provider and actual query, describes keyword/index coverage, and sets `scientific_suitability_assessed` and `proves_catalogue_absence` to false. These flags describe what the search adapter establishes; subsequent inspection and researcher review remain separate. Do not infer specific upstream BioArt index fields from this general scope description.

`cargo run -p bioicons --example search -- RAS 12` performs a read-only catalogue search without downloading an icon. An optional third argument is the exact author name, for example `cargo run -p bioicons --example search -- protein 12 "AUTHOR FROM A RESULT"`. This is an attribution filter, not an artwork recommendation. The existing `inspect` example additionally downloads and normalizes the first result for technical inspection; that first result is not an automatic scientific recommendation.

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


### SVG 内部路径样式

`parts::inspect` 为当前文件快照返回最多 256 条非资源路径、稳定编号、原 SVG 标识（若有）、填充/描边、边界和编号 PNG。编号由当前文件哈希及顺序构成，不是生物学身份，采用修改后须重新读取。裁剪、遮罩和图案定义中的共享路径不列为编辑组件；复杂素材明确报错，不截断路径列表。

`parts::edit` 校验原文件哈希及 1..32 条唯一的路径修改，只接受填充/描边 `#RRGGBB` 或 `none`，以及 0..1 的局部透明度系数、`translate:[dx,dy]` 的平移。平移单位是规范化 SVG 根坐标，不是画布像素，后台补偿上层旋转、非等比缩放和镜像；渐变随路径移动，上层共享裁剪与 SVG 视口保持原位，超出部分仍会被裁掉。位移须为有限数且每轴不超过 ±100000；不可逆变换或过大的局部变换被拒绝。系数相对当前效果叠加：1 保持、0 隐藏；分组的合成透明度继续生效，恢复原效果可使用原文件/图稿历史。未指定属性、其他路径及仍在使用的渐变/裁剪定义保持原有语义。替换某条路径的渐变填充为实色是显式操作，不影响其他渐变使用者。文字须已转为轮廓；该接口不会因换色替换字体。

路径仍在一个可缩放的 SVG 图片对象中，已提供独立路径平移；当前不提供控制点、路径形变或拆成原生画布对象后的拖动。带编号 PNG 是审阅材料，不写入正式素材。测试覆盖同色路径隔离、变换后边界、共享资源、真实受体与文字轮廓、旋转/缩放/镜像下平移、渐变跟随及固定共享裁剪的像素验证、透明度像素、快照过期、非法操作和容量限制。


### 端点与 Bezier 控制点

`geometry::inspect(bytes, sha256, part_id)` 返回规范化 SVG 当前路径的端点/控制点编号、SVG 根坐标、M/L/Q/C/Z 命令结构与编号 PNG。编号依赖当前文件快照。`parts::Edit.points` 是稀疏的 `[{index,position:[x,y]}]` 修改，保持未指定点与路径命令结构；单条路径最多 512 个点，一次最多修改 64 个不同编号。坐标须为有限数且在 ±1000000 内；无法稳定逆变换的局部坐标会被拒绝。超限路径明确标出不可编辑，不截断点列表。

每个点在 SVG 根坐标中定位，后台逆变换回所属路径坐标，支持旋转、镜像与非等比缩放。控制点修改不自动带动相邻端点/控制点，不自动平滑曲线，不移动共享绘制资源或上层裁剪；形状改变后仍使用原有绘制坐标。一个路径的 `points` 与 `translate` 须分两份提案采用，避免混淆绝对位置与相对位移。归一化后会再次核对路径数量和命令拓扑，退化导致路径消失或结构变化时拒绝修改。当前不增删点、子路径或命令。

Path inspection returns `bounds_kind: "svg_root_stroke_aabb"` and a `bounds_note` alongside path boxes. `bounds` already include path strokes and ancestor transforms in normalized SVG root coordinates; callers should not add the stroke width again. These are geometric boxes, not exact clipped-pixel masks or biological-domain annotations. The additional result fields do not change snapshot IDs or edit semantics.

“Normalized SVG root” refers to the canonical SVG coordinate system, not a unit interval. Use the returned `width`/`height` when converting path boxes to 0..1 fractions for an illustration-anchor operation.

Prepared vectors are reused in a client-local LRU shared by cloned clients: at most 16 entries and 32 MiB of encoded SVG plus preview payloads (excluding metadata and allocator overhead). A retained entry preserves its original download hash, normalized bytes/hash and PNG preview exactly; repeated preview/insertion calls do not re-download or re-normalize it. This is a bounded in-memory snapshot, not a live freshness guarantee: eviction or a new client permits a fresh fetch. Errors and oversized payloads are not cached. Concurrent cache misses may independently fetch the same asset; the cache lock is not held across network or rendering work. Catalogue and per-icon licence provenance remain separate and unchanged.


路径检查返回 `stroke_width`（规范化 SVG 局部单位，解析路径无描边定义时为0）；`parts::Edit.stroke_width` 可显式指定有限的 `(0,1000]` 宽度，省略则保留。改变宽度不修改路径控制点、填充或颜色，未描边路径还须显式设置stroke。宽度受路径变换和画布缩放影响，已有裁剪保持，加粗可能扩大范围或被截断，须查看预览。使用stroke:none隐藏描边；0宽度请求拒绝。
