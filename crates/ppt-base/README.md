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
  tools/           13 个 agent 工具(deck_* / slide_* / asset_attach /
                   deck_export / deck_references)
```

## 渲染与导出约定

- **单一渲染路径**:GUI 查看器与导出打印页共用 `apps/gui` 的
  `features/deck` 渲染器;导出经 `/deck/print`(一次性 print token,
  15 分钟,sha256 存储)驱动 headless Chromium。
- **等待协议**(slidev 移植):`<html data-deck-ready>` + `[data-waitfor]`
  哨兵排空 + mermaid/图片 settle;`window.__deckGoto(slide, click)` 供
  逐页捕获。
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

## 配置

- `deck.db`(state_dir),env `AUTONOMICS_DECK_DB`,builder
  `RuntimeConfig::builder().deck_db_path(...)`
- 测试隔离:`gateway::testing::isolate_runtime_paths` 覆盖 `deck.db`
- 导出地址:与 daemon 绑定同源(`AUTONOMICS_HTTP_API_ADDR`)

## 测试

```
cargo test -p ppt-types
cargo test -p ppt-base        # 83 单元 + tools_e2e + citation_e2e
cargo test -p api-server      # deck REST(含 print token 面)
cd apps/gui && npm run test:deck   # mock-gateway Playwright
```

(不要 `cargo test --workspace` —— 本机内存会 OOM,按 crate 串行。)
