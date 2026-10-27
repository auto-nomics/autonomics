# P3 导入与发现工作区本机验证

日期：2026-10-28。P3 的本地阶段功能与验收已完成；分次资格记录保留各自范围，本记录不等于生产发布验收。

## 库内匹配与导入规模

使用现有 P1/P2 `search_benchmark` 合成数据集的**私有数据库副本**，经生产 `BibBase` actor 执行。原始 fixture 不写入。使用 release 构建、Linux x86_64，每类 20 次；文件复制和初次索引构建不计入导入时延，操作系统缓存为热缓存。未隔离整机其他负载。

| 项目 | 10k 文献 | 100k 文献 |
| --- | ---: | ---: |
| 1 项已有文献预览 P95 | 34.7ms | 557.3ms |
| 100 项已有文献预览 P95 | 52.0ms | 574.1ms |
| 100 项、半数新文献预览 P95 | 43.2ms | 584.9ms |
| 100 项首次记录导入审计，单次 | 72.3ms | 626.8ms |
| 100 项幂等重复导入 P95 | 68.3ms | 621.6ms |

基准逐项断言：大小写/DOI URL 形式匹配到预期本地 ID，新旧混合预览正确，已有元数据不被替换，重复导入不再产生变化，文献总数保持不变。首次导入的记录均为已有文献，只添加审计来源；未把“新建 100 篇文献”的写入成本混入该项。

当前实现每个预览/导入批次扫描一次当前库的标识符，内存只保留候选涉及的键；同批独立事务通过 revision/token 缓存复用匹配结果。100 项与 1 项的接近耗时验证避免了逐项全库扫描，同时也暴露 100k 单项约半秒的固定扫描成本。尚未建立覆盖所有既有数据的常驻规范化 ID 索引。该结果不能称为 O(1) 查询，也不代表 HTTP/GUI 端到端或真实库相关性。

进程峰值 RSS 约 185/451 MiB，包含初次索引构建的进程峰值；测量结束 RSS 约 125/190 MiB，不能将峰值单独归因于导入。完整数据见 [10k 收据](literature-search-p3-10k-import-release.json) 和 [100k 收据](literature-search-p3-100k-import-release.json)。

复现：

```bash
cargo run --release -p bib-base --example discovery_import_benchmark --locked -- \
  --fixture /path/to/closed/search_benchmark_fixture --repeats 20 --output report.json
```

要求 fixture 含 `library.db`，WAL 已 checkpoint（若含未合并帧则拒绝复制）。程序在临时目录创建副本并记录其路径，方便检查审计结果。

## GUI 与原生传输

- `npm run test:bib-discovery`：Chromium 浏览器与模拟 Tauri 桥接，验证显式提交、逐源交付、稳定选择、指纹冲突、版本选择、导入、只读目标、工作区状态、窄窗口、快捷键和 UUID 回退。模拟桥接不视为实际 WebView。
- `npm run test:bib-discovery-native`：实际 Linux Tauri WebKitGTK，经过 native IPC、鉴权本地 HTTP API、会话、数据库与 GUI，完成选来源版本、预览导入、库内刷新、删除会话后查看持久化来源。结果见 [原生收据](literature-search-p3-native-webview-linux.json)。来源为确定性 fixture，不发起真实上游检索。
- 后端 32 项 discovery 单元测试、5 项 discovery API 测试、4 项来源 HTTP 契约测试通过。扩大 Rust 回归 276 项通过，此后新增的第 5 项 API 用例单独通过。
- 后端库严格 Clippy、原生所有 target 的严格 Clippy、新基准与 API 用例严格 Clippy、前端构建通过。后端所有 target 的扩大检查遇到原有 `stored_files.rs` 的 `items_after_test_module`，未记录为通过。

其后已补齐五源资格和分页/取消/容量 GUI 回归，见下文收口记录；macOS/Windows、人工屏幕阅读器和真实库相关性/端到端时延仍为发布门槛。

## arXiv 后续资格

新增的 arXiv 适配通过 7 项受控 HTTP 用例（来源 HTTP 总计 11 项），包含真实 SDK 翻页、版本约束、原生 ID 交集、窗口边界、总数漂移、错误/零结果、取消关闭响应连接及旧通用年份不发网络请求的断言。SDK 另有串行连接、请求起始间隔、Retry-After、长查询 POST、非 Content-Length 正文上限、Atom 完整性及窗口边界验收；38 项 SDK 单元用例、5 项 doctest 通过，8 项在线测试未执行。

该次 arXiv 资格的发现核心为 36 项相关单元测试，发现 API 为 5 项。GUI 测试覆盖单选原生来源、逐源查询草稿和认证要求说明；实际 WebKitGTK 收据增加了 native IPC → 本地鉴权 API → 实际 arXiv SDK → 本地 HTTP/Atom → 结果的路径。外部响应受控，不属于真实上游或相关性验收。完整命令和检查见 [arXiv 资格收据](literature-search-p3-arxiv-linux.json)。

## Crossref 后续资格

新增 Crossref 适配和共享 SDK 的隔离 HTTP 测试。SDK 22 项单元、6 项 HTTP 契约、3 项 doctest 通过，7 项在线用例忽略。发现核心增至 42 项，来源 HTTP 增至 19 项，发现 API 仍为 5 项，均通过。

覆盖：DOI 单例/404/身份不符、精确 DOI 组合与年份/类型传输、近似匹配显式选择、跨字段 OR 拒绝、NOT 保留、原生参数和凭据保护、共享 clone 限速、响应头与 Retry-After 日期/秒数、冷却溢出、完整正文期间的连接槽位、固定/分块正文上限、错误响应不变成零结果、重复 raw cursor、游标绑定/过期/大小、取消关闭实际 HTTP 响应连接。

浏览器与模拟桥接新增分页、容量失败保留候选、单源重试复用会话，及两项真实状态回归：新查询移除来源后清理旧结果筛选；重连只比较 batch 自身的 sequence，历史 batch 加上新的 source_started 不能提前提示有新结果。

实际 Linux Tauri WebKitGTK 经 SDK 和受控 HTTP 端点获取 Crossref 25+1 条记录，点击来源续页，刷新后滚动虚拟列表到末尾验证新增行，并确认短页后不再提供来源续页。原生用例此前暴露了历史 batch 重放造成过早刷新的问题，修正后完整流程通过。原生证据见 [Crossref WebView 收据](literature-search-p3-crossref-native-webview-linux.json)。

SDK 所有 target、后端库、来源 HTTP target、原生 example 严格 Clippy，目标 Rust 格式检查和前端构建通过。命令与结果见 [Crossref 资格收据](literature-search-p3-crossref-linux.json)。这些是该次 Crossref 本地确定性资格，未代替其他来源或发布验收。

## Semantic Scholar 后续资格

SDK 22 项单元（含旧摘要序列化兼容）、7 项受控 HTTP 契约、2 项 doctest；发现核心 49 项、发现 API 5 项、两个来源 HTTP target 共 28 项通过。21 项 S2 在线用例未运行。SDK 所有 target、后端库、来源 HTTP target 与原生 example 严格 Clippy，目标 Rust 格式检查、前端构建和浏览器/模拟桥接回归通过。命令与检查见 [S2 资格收据](literature-search-p3-s2-linux.json)。

协议覆盖真实 bulk token、批次内小页缓冲、年份/类型/venue 和排序参数、relevance offset/窗口/总数变化、精确 ID 身份验证、坏记录/重复 token/错误封装、取消关闭连接、重试预算和配置凭据保护。SDK 另外验证空闲后 clone 不突发、Retry-After 日期/秒数/溢出、成功/错误正文大小、自定义客户端超时、上游 abstract 字段映射。旧 StructuredSearch 布尔与元数据组合实际走 bulk，不把不可支持的字段折为正文。

每个 S2Source 的批次缓存最多 32 条目、32 MiB 序列化预算；不将其当作 RSS，也不混入会话候选预算。测试验证有界淘汰与非 LRU 顺序的到期清理；模拟淘汰后真实 HTTP 重取相同批次可继续，源元数据改变则过期。原始上游元数据参与指纹，本地创建时间不参与；执行信息标注批次实际取得时间。

实际 Linux Tauri WebKitGTK 完成 SDK HTTP 原生布尔查询与年份筛选，取得 30 条批次后关闭测试端点，仍可按 GUI 25+5 条继续读取并滚动到末尾。说明续页使用缓存而非再次请求已关闭端点。证据见 [S2 原生收据](literature-search-p3-s2-native-webview-linux.json)。GUI 同时区分 Source order 和仅排列已取得候选的结果顺序。

P3 已完成阶段统一审查与本地受影响回归，形成一次阶段提交。完整回归 319 项通过；3 项需 Node 环境的提取/API 用例另行通过，原生 HTTP 传输 3 项通过。1001 个候选的分页、过期快照、虚拟化和键盘回归通过；独立暂存源码的构建、浏览器/模拟桥接与实际 Linux Tauri 流程通过，提交的前端包由该独立源码生成。收口证据见 [阶段收据](literature-search-p3-stage-linux.json)。真实库相关性/线上上游表现、跨平台及人工可访问性继续保留为发布门槛。
