# 直接画布编辑验收

现行测试不再使用候选创建、分支修订或人工采用。

| 范围 | 回归位置 |
| --- | --- |
| 即时保存、连续修改、原子失败、并发旧版本拒绝 | `visualization-tools/tests/direct_canvas.rs` |
| 局部文字/几何修改与 SVG 来源保留 | `visualization-tools/tests/direct_canvas.rs`、`visualization-assets/tests/colors.rs` |
| 图形、连接线、分组、裁剪、标签和布局不变量 | `visualization/tests/` |
| HTTP 当前画布可见与旧路由消失 | `api-server/src/visualization.rs::direct_tests` |
| 真实 Excalidraw 同步、当前 PNG、源文件、撤销和自动保存 | `apps/gui/tests/canvas-direct.mjs` |
| 宿主工具循环与视觉消息 | `gateway/tests/visualization_conversation.rs` |

GUI 测试使用确定性 HTTP 数据模拟 Agent 外部写入；底层工具与 API 分别执行真实 SQLite 操作。它们不衡量真实模型的科学推理、审美质量或稳定提速。真实模型的端到端质量与延迟需要另做对照评估。
