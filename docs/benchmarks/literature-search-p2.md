# P2 带分面和当前页元数据的服务端基准

P2 工作区实现的诊断收据，尚不代表 GUI 阶段完成。使用与 P1 相同的混合中英文/科研符号合成库，`--reuse true --gui true --query-repeats 20`。GUI 模式请求 `include_facets:true, include_articles:true`，涵盖精确分面计数和当前页 50 篇的批量元数据。没有浏览器渲染、HTTP 网络和 Tauri 传输成本。

```sh
cargo build --release -p bib-base --example search_benchmark
target/release/examples/search_benchmark --count 100000 --directory EXISTING_SYNTHETIC_FIXTURE --reuse true --gui true --query-repeats 20 --output docs/benchmarks/literature-search-p2-100k-gui-release.json
```

原始逐篇 SQL 分面读取的 `*-gui-uncached-release.json` 暴露纯 NOT 宽查询成本。改为 revision/token + outbox 逐文献失效的惰性元数据缓存后，`*-gui-release.json` 保留最终对比。缓存不改变计数、排序和筛选语义，也不缓存全文。元数据缺失时仍需首查加载，因此同时报告首次和热查询。

| 文献 | 纯 NOT 未缓存 P95 | 缓存后纯 NOT P95 | 缓存后该类别首查 | 标题+年份 P95 | 标题短语 P95 | 峰值 RSS |
| --- | --- | --- | --- | --- | --- | --- |
| 10k | 133.03 ms | 16.53 ms | 94.48 ms | 12.50 ms | 5.41 ms | 202.7 MiB |
| 100k | 1372.12 ms | 202.37 ms | 1025.77 ms | 286.13 ms | 46.98 ms | 502.2 MiB |

两种规模均通过 100 个 DOI Top-1 检查和各类别总数稳定性检查。覆盖修改、删除、提取成功/重试/失败后缓存失效的回归位于 `tests/search_workflow.rs`。同一文章多个附件可同时提供 indexed/failed 状态；分面不隐藏部分失败。

100k 宽查询首查仍约 1.03s，不能用热 P95 掩盖。这是惰性加载较大候选元数据的成本，GUI 应明确加载状态并支持取消；如果发布要求首次宽查询也低于 400ms，则需继续预热或将精确筛选/分面下推索引。当前目标中的热查询指标已满足该合成样本，但真实相关性、端到端交互、冷 OS 缓存、跨平台与高并发仍需另行验收。硬件/非独占环境及采样局限沿用 P1 基准说明。


## GUI 与原生传输资格

`npm --prefix apps/gui run test:bib-workflow` 分别覆盖浏览器和模拟 Tauri bridge 的高级条件、精确分面、规则保存/冲突、URL 重载、多附件证据、取消和虚拟列表键盘选择。模拟 bridge 用于前端分支契约，不能作为实际原生运行证据。

实际 Linux Tauri WebKitGTK 的独立 smoke 使用生产 IPC 命令、reqwest、真实 API server、隔离数据库和两份原始 TXT 附件，完成检索、筛选、保存规则、打开非首选命中附件、文本定位与返回保留查询。命令为 `npm --prefix apps/gui run test:bib-native`，原始收据见 [Linux 原生 WebView](literature-search-p2-native-webview-linux.json)。Rust 传输测试另验证取消使真实 HTTP 连接关闭，以及附件 key/hash/auth 的请求路径。

阅读器/文献菜单/集合浏览回归串行运行，避免多个 Vite 开发服务器同时写优化缓存造成测试干扰。自动化检查覆盖 roving focus、虚拟行重挂载、勾选状态、对话框焦点和 reader Tab trap；未据此宣称通过人工屏幕阅读器测试。macOS/Windows 实际执行与人工可访问性验证仍为生产发布门槛。

旧 SQL 词项表不再在普通源写入时分词双写；显式 legacy 模式在检索事务中按需重建/重放。兼容入口对照与回退恢复测试分别位于 `tests/search_workflow.rs`、`legacy_index::tests`。

最终收口复测更新了缓存后两份 JSON；未缓存列保留历史诊断值。机器非独占，复测的 100k 标题+年份 P95 为 286.13ms，不能沿用先前 170.98ms 作为最终值。样本仍为每类 20 次；这组收据不代表真实库相关性或操作系统冷缓存。
