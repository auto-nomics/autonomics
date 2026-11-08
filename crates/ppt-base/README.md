# ppt-base / ppt-types — Slide-Deck Engine

以 [Slidev](https://github.com/slidevjs/slidev) v53.1.0(MIT)语义为蓝本的
**Rust 全自研幻灯片引擎**,面向生物医学科研场景(组会/文献报告、答辩/会议
演示、投稿配图/PPT 交付),服务于 agent:agent 写 markdown,引擎负责解析、
存储、渲染载荷与导出。

## 架构

```
crates/ppt-types   纯数据模型(serde camelCase = wire 格式)
                   DeckMeta / SlideSpec / SlideFrontmatter / ClickPlan
                   Layout 枚举 / AspectRatio / 页区间解析

crates/ppt-base
  parser/          Slidev parser 移植:行扫描器(frontmatter/code-fence/
                   HTML 注释规则)、speaker note、标题/图片提取、
                   katex/mermaid 特性检测、round-trip stringify/prettify
  deck.rs          组装:headmatter defaults ⊕ slide frontmatter、
                   ClickPlan 计算、DeckMeta/SlideSpec 生成;生物医学模板
                   (meeting / defense / figures)
  operations.rs    slidev MCP 模型的源操作:slide_patch / insert /
                   remove / move(headmatter 守卫、YAML 先验校验、
                   未改页 raw 逐字保留)
  store.rs         Turso 持久化:decks / deck_versions / deck_assets /
                   deck_exports;parsed_cache 按 revision 失效
  assets.rs        blob 层:vfs OpendalFileStorage(deck-assets /
                   deck-exports 前缀)+ 内存后端
  export/          ChromiumExporter(打印页 + 等待协议 + printToPDF /
                   截图)、ExportQueue(单 worker)、PPTX 最小 OOXML
                   writer(全幅图 + 真 notes)
  citation.rs      {{cite:key}} 扫描 + bib-base 解析 → References 页
  tools/           agent 工具(deck_* / slide_* / asset_attach /
                   deck_export / deck_references)
```

## 渲染与导出约定

- **单一渲染路径**:GUI 查看器与导出打印页共用 `apps/gui` 的
  `features/deck` 渲染器;导出经 `/deck/print`(一次性 print token,
  15 分钟,sha256 存储)驱动 headless Chromium。
- **等待协议**(slidev 移植):`<html data-deck-ready>` + `[data-waitfor]`
  哨兵排空 + mermaid/图片 settle;首先等待 `window.__deckGoto` 注册，
  翻页时同步重置 ready，并等待 `window.__deckState` 与目标页和 click
  一致后再捕获；打印数据加载失败通过 `data-deck-error` 立即报告。
- **Click 渐进**:Rust 侧整数代数(slidev useClicks 语义)算出
  `ClickPlan`,前端只消费结果;`<v-click>` 与 `v-click="…"`
  两侧扫描器模式一致(顺序对齐)。
- **PDF**:`mode=pdf` 堆叠全部页 + `@page` 精确尺寸 → 一次
  `printToPDF` 自然分页;**PNG**:DPI 缩放(期刊 300 DPI)+ clip 截图
  打 zip;**PPTX**:最小 OOXML,全幅图 + 真备注(sldSz EMU = px×9525)。

## Agent 工具面

`deck_create`(meeting/defense/figures 模板)、`deck_list/get/outline/
delete`、`slide_patch/insert/remove/move`、`asset_attach`
(vfs_path 优先,caption + provenance)、`deck_export` /
`deck_export_status`、`deck_slide_preview`(单页渲染 PNG 以图像块
回读,click 状态可选,agent 唯一"看见"成品的方式)、`deck_references`。
能力门控:与 writing 同挂 `AgentKind::enable_writing()`。

## CSS 主题与页面覆盖

每个 deck 在素材存储下有独立的 `themes/{deck_id}/theme.css`。新建 deck
时写入初始页面尺寸；旧 deck 缺少文件时按空主题读取，首次保存时创建。GUI 的
“CSS 样式”可编辑整套主题，或选择某个可见页编辑其覆盖样式。未保存的草稿
在切换预览和源码时保留；保存采用 revision 检查，冲突时不会覆盖新版本。

新建 deck 默认使用生物医学科研主题：深蓝封面、白色内容页、青绿色重点、
克制的表格分隔线以及用于参考文献和图版的布局。主题 CSS 会随新建 deck
复制到其独立文件中，之后可编辑；已有 deck 的主题不会被覆盖。已有 deck
可在 CSS 编辑器中选择“载入默认主题”，检查草稿并保存后应用，原页面尺寸
会保留。字体采用系统无衬线字体并提供中文回退，不依赖在线字体。

页面覆盖存放在该页 frontmatter 的 `slideCss` 字段中，因此随页面移动、删除
以及 Markdown 版本快照一起变化。保存只修改该字段，保留其他 frontmatter
的内容与注释。普通声明使用 CSS cascade layer 依次应用内置样式、主题和页面覆盖；
`!important` 的层顺序遵循 CSS 标准而反转。例：

```css
/* 整份演示文稿的物理页面尺寸；两个值必须一起设置 */
:root { --deck-width: 1280px; --deck-height: 720px; }
/* :scope 是当前页面根节点 */
:scope { background: #f7f9fc; color: #17324d; }
.deck-prose h1 { color: #3158b8; }
```

CSS 选择器限定在对应页面范围，动画和自定义字体名称按渲染页隔离，预览和 PDF/PNG/PPTX 导出共用同一套渲染；
尺寸变量须在主题的顶层 `:root` 或 `:scope` 中设置，支持 16–8192 的整数 `px`；
不设置时沿用 Markdown 中的 `canvasWidth` 和 `aspectRatio`。页面覆盖不能改变物理尺寸。
创建导出任务时会固定页面尺寸和主题 CSS，避免排队期间修改主题导致截图和 PPTX 的尺寸不一致。
不支持 `@import`。图片或字体可以使用已附加素材的 `assets://{id}` 引用，
打印时会转换为 token 授权地址。主题文件与 Markdown 分别保存和检查 revision，
Markdown 历史版本不会回滚主题文件。特殊的 flow-style frontmatter 或缩进顶层
`slideCss` 无法安全原位修改时，编辑器会报错，需先改为普通顶层键。

Agent 可通过 `deck_css_get` / `deck_css_set` 读取和保存主题或单页 CSS；
后者要求传入前者返回的 revision。API 使用 `GET/PUT /decks/{id}/theme.css`
和 `PUT /decks/{id}/slides/{no}/css`，页面编号按可见页面计算。

## 层级大纲与章节导航

每页默认使用首个 Markdown 标题的 `#` 层级；frontmatter 的 `level: 1..6`
可以显式覆盖。封面不作为章节父节点。比上一页层级更深的标题归入最近的
较浅标题；左侧“页面目录”按层级缩进，带子页的条目可以展开或折叠。
`hideInToc: true` 隐藏目录项，但页面仍存在且可以播放。

`layout: section` 或 `section: true` 标记章节起点，未指定 `level` 时
默认为一级；具有子页的标题也会自动识别为章节。`section: false` 可以
阻止某个标题自动成为章节。显式章节之后的普通页会归入该章节，即使每页都使用
`#` 作为标题；下一个显式章节或手动指定的 `level` 可以改变层级。
`sectionTitle` 可为导航栏提供较短的名称：

```markdown
---
layout: section
level: 1
sectionTitle: 方法
---
# 研究方法

---
level: 2
---
## 队列与研究设计

---
layout: section
level: 2
---
## 统计分析

---
level: 3
---
### 敏感性分析
```

幻灯片内章节栏默认隐藏，不占用内容空间。在主题 CSS 中启用：

```css
:scope:not([data-layout="cover"]):not([data-layout="section"]) {
  --deck-section-nav-display: flex;
  --deck-section-nav-color: #526a7b;
  --deck-section-nav-active: #11767b;
  padding-top: 88px; /* 为章节栏留出空间 */
}
:scope:not([data-layout="cover"]):not([data-layout="section"])::before {
  display: none; /* 默认主题的顶部装饰线 */
}
```

默认主题文件中也提供了上述可取消注释的示例。章节栏列出顶层章节并高亮
当前页所属章节，子章节以路径显示；预览中可点击章节跳转，导出中保留同样的
视觉效果。可使用 `.deck-section-nav`、`.deck-section-nav-link`、
`.deck-section-nav-link[data-active="true"]` 和 `.deck-section-breadcrumb`
进一步调整样式；导航容器的 `data-current-section` 是当前顶层章节的页码，
条目的 `data-section-index` 是从 1 开始的章节序号。单页覆盖 CSS 可以用 `:scope { --deck-section-nav-display: none; }`
关闭该页导航。`deck_outline` 和 REST 返回的 `outline` 使用相同的层级与章节路径。

## 美学诊断 (Aesthetics diagnostics)

Every write operation (`save_markdown` / `save_theme_css` — i.e. all slide
ops, source edits, theme and per-slide CSS) fires a non-blocking notify
into a single-worker diagnostics queue that coalesces bursts of same-deck
writes into one run against the latest state. Reports land in the
`deck_diagnostics` table (latest per deck), keyed by markdown revision +
theme revision so readers can tell fresh from stale.

Two tiers, one report:

- **Static** (Rust, always runs): markdown-level rules — `empty-slide`,
  coarse `text-density`. Works even without a render backend.
- **Render** (headless Chromium, same exporter as preview/export): the
  print page measures each slide's real DOM at scale 1.0 through
  `window.__deckDiagnose()` (`apps/gui … render/diagnostics.ts`, the
  single render path). Rules: `text-overflow-canvas` (error; text ink
  extent via `Range`, so nowrap overflow past the border box is caught),
  `image-overflow-canvas` (error), `content-clipped`, `font-too-small`
  (floor scales with canvas width, 14px @ 1280), `low-contrast`
  (WCAG ratio < 3:1 against the nearest opaque ancestor background;
  image/gradient backgrounds are skipped, not guessed), `text-density`
  (visible-text measurement), `empty-slide`. Render findings supersede
  static duplicates by `(slide, rule)`.

Surfaces:

- **Write-tool result injection**: `slide_patch` / `slide_insert` /
  `slide_remove` / `slide_move` / `deck_css_set` results carry a
  `diagnostics` block — static-tier findings for the just-saved state
  (synchronous, microseconds) plus the render tier's status
  (`scheduled` → call `deck_diagnose` for the full layout report).
- Agent tool `deck_diagnose` — serves the stored report when fresh,
  otherwise re-runs synchronously (`refresh: true` forces a rerun).
- REST `GET /decks/{id}/diagnostics` (stored report + freshness) and
  `POST /decks/{id}/diagnostics` (forced synchronous run).
- The deck payload carries a `diagnostics` summary badge
  (fresh/rendered/error/warning/info counts) for the GUI.

When no render backend is available (tests, NullExporter) the report
degrades to the static tier with `rendered: false`.

## 配置

- `deck.db`(state_dir),env `AUTONOMICS_DECK_DB`,builder
  `RuntimeConfig::builder().deck_db_path(...)`
- 测试隔离:`gateway::testing::isolate_runtime_paths` 覆盖 `deck.db`
- 导出地址:与 daemon 绑定同源(`AUTONOMICS_HTTP_API_ADDR`)

## 测试

```
cargo test -p ppt-types
cargo test -p ppt-base --features vfs
cargo test -p api-server      # deck REST(含 print token 面)
cargo test -p ppt-base --features vfs --test diagnostics_e2e
cargo test -p ppt-base --features e2e --test chromium_readiness_e2e # 需要 Chromium
cd apps/gui && npm run test:deck   # mock-gateway Playwright
```

(不要 `cargo test --workspace` —— 本机内存会 OOM,按 crate 串行。)
