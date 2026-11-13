# Figure asset operations

Shared API/agent layer over `visualization` storage and `bioicons` SVG processing. The storage crate remains independent of the catalogue and renderer. These operations use saved, project-scoped files and do not access the network.

- `palette(store, project, figure, revision, target)` reads actual SVG paint colours and returns a PNG preview. `target` can be an image ID or its existing semantic anchor. It requires the exact current revision.
- `recolor(...)` creates a reviewable proposal from an exact colour map (`#RRGGBB` to `#RRGGBB`). `recolor_document(...)` exposes the pure document transform for hosts that manage their own review flow.

Replacements apply simultaneously to matching fill, stroke, gradient-stop, flood and lighting colours. Invalid or absent source colours fail before proposal creation. Geometry, glyph paths, opacity and unrelated objects remain unchanged. A new content-addressed file is referenced only by the chosen image instance; other instances and original file records retain their original colours. Immutable figure revisions preserve the old artwork.

The image retains original author/licence/source metadata and arbitrary user notes. Its `modifications` list appends the exact map, previous file ID and SHA-256, resulting SHA-256 and timestamp. The upstream `source_sha256` is preserved; imported SVGs without that field record the existing embedded file hash as their starting source. None of this asserts a licence or scientific relationship not already supplied by the user/source.

API:

- `GET .../figures/{id}/vector-palette?element_id=...&expected_revision=N`
- `POST .../figures/{id}/vector-colors` with `{element_id,expected_revision,colors:{"#ff0000":"#00aa88"}}` (actual values must be six hexadecimal digits).

The GUI's “素材编辑” control reads one selected SVG, allows exact colour substitution, and opens the canonical before/after proposal viewer. It checks local edits and revisions while reading and proposing. The `bioicons` tool exposes equivalent `palette` and `recolor` actions, with a vision block for the palette preview. PNG images are not supported by these colour operations. Palette replacements change matching colours together. Path-specific editing uses the separate parts/edit_parts operations below.

`cargo test -p visualization-assets` verifies instance isolation, preserved metadata/geometry, immutable history, version conflicts and failure atomicity. The live BioIcons browser case additionally covers palette selection, rejected stale/invalid requests, proposal adoption, reload and vector export.

Normalized SVG uses an editor-compatible root namespace/viewBox and document serialization so file loading does not change content-addressed bytes. Browser validation checks the hash of the inserted and reloaded bytes against the recorded file hash.

配色要求 SVG 内文字已转为路径；BioIcons 导入时已处理该转换。含原始文字的其他导入 SVG 会明确报错，避免配色动作悄悄替换字体或改变字形。


`parts` 和 `edit_parts` 在相同的保存版本及目标图片/语义锚点上提供逐路径样式和平移编辑，使用 `bioicons::parts` 的快照哈希和路径编号。更新仍只替换当前实例文件引用，保留布局、来源许可、共享原文件及历史修订，追加 `path_style` 或包含平移的 `path_edit` 修改记录（路径 ID、样式、位移、前后哈希及时间），结果为待审阅提案。GUI 与 Agent 共用这些函数，无须目录或网络访问。


`geometry` 按同一版本、目标图片、文件 SHA 与路径 ID 返回端点/控制点及编号视觉材料。端点编辑沿用 `edit_parts` 的提案、单实例替换、不可变历史和来源记录机制，新增 `points` 记录进入 `path_edit` 历史；不改变外部标签、箭头、裁剪或其他素材实例。


`image_info` reads actual embedded SVG/PNG/JPEG/WebP dimensions, current crop, SHA-256 and provenance at the exact saved revision. `inset` compiles a fractional visible region into a pending editable crop/region/callout proposal, preserving source objects and files. `inset_status` compares the source with its recorded historical version; a later figure revision alone is not considered a source change. `refresh_inset` retains inset identities, panel, title, styles and local notes while proposing refreshed content and region/callout geometry inside the existing panel. The same fractional region is applied to the current source viewport and needs scientific review if the source content changed. Local file/crop edits require `discard_detail_edits:true`; missing sources/members, ambiguous identities and rotated panels fail before proposal creation. The source and unrelated insets remain unchanged. Every refresh records current and previous source revisions/file hashes, and remains a fixed snapshot until explicitly adopted and refreshed again.

HTTP: `GET .../image-info`, `POST .../insets`, `GET .../inset-status`, `POST .../insets/refresh`. Read queries use `element_id` and `expected_revision`; refresh accepts the same fields plus optional `discard_detail_edits` (false by default). Native editor Delete uses ordinary engine rules; the GUI's reviewed delete operation and Agent removal of the inset anchor also clean reciprocal region/callout decorations.

## 在候选图上继续插图化

`candidate::replace` 接收现有候选 ID、精确文档哈希、正式基础版本、目标对象和已下载的图片，创建具有父来源记录的子候选。替换基于父候选文档，因此此前的文字、布局、文件及其他素材修改得以累计；父候选和正式图稿不变。保留语义锚点、标签身份、原有关系绑定及科学依据。图片的来源、作者、许可和哈希仍由具体目录适配器提供。

父候选必须属于当前项目/画布，仍待审且基础版本和哈希完全匹配。有关联的开放审阅时，必须显式给出 `markup_ids`：提供 IDs 表示继续关联，`[]` 表示明确解除；省略不会悄悄丢失关联。操作不完成审阅，也不自动采用。三项集成测试覆盖连续两次插图化、历史和无关元素保留、正式版本隔离，以及过期/错误来源和审阅失败时不写入。

`candidate::inspect_svg` 和 `candidate::refine_svg` 将相同的配色、路径与控制点能力用于待审候选，无须先采用。HTTP 的 `vector-palette`、`vector-parts`、`vector-geometry` 查询，以及 `vector-colors`、`vector-parts` 写入均可同时提供 `proposal_id` 与 `document_sha256`；省略两者表示正式版本，只提供一项则报错。检查结果包含实际作用域、候选文档哈希和文件身份，错误候选不会回退读取正式图。只读查询拒绝 `markup_ids`；写入遵循上述显式审阅关联规则。路径编辑的 `sha256` 是当前 SVG 文件哈希，与候选 `document_sha256` 不同；每次编辑产生新文件后应重新读取路径编号。核心和注册工具集成测试覆盖正式图尚无素材时的检查、连续配色与路径编辑、来源保留、过期路径拒绝以及父候选隔离。

`candidate::insert` 在相同的引用/版本/审阅检查下追加独立素材，返回子候选和新图片 ID。它不替换或重建已有节点，不修改科学关系，位置使用场景 x/y。连续插入保留父候选的所有元素、文件及前序修改；同文件复用字节但为每个实例创建新对象 ID。核心测试覆盖连续追加、独立位置和来源、不可变祖先、显式审阅关联，以及非法范围/坐标/已处理候选拒绝后无写入。

SVG `image_info` additionally returns `file_id` and `svg_preview_alpha_bounds`, measured from the prepared preview (at most 768 pixels per side). The nonzero-alpha bounding box includes opaque backgrounds and decorations; normalized bounds add one preview pixel of padding. Empty SVGs return null bounds. Match `file_sha256` to the current file bytes before using this hint. Coordinates cover the whole uncropped source; native crop, rotation, mirroring and element opacity are not applied. This is sampled layout guidance, not exact vector geometry or scientific validation; review the rendered candidate before fitting. Raster files retain dimensions-only inspection.
