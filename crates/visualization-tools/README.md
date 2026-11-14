# visualization-tools

Agent 直接读取和修改 GUI 共用的项目画布。没有候选、分支、采用或拒绝阶段。

## 编辑协议

1. `visualization read` 获取 `project_id`、`figure_id`、当前 `metadata.revision` 与对象。可用 `element_ids` 缩小读取范围，图片字节默认省略。
2. `visualization apply` 传 `expected_revision` 和 `operations`，一次原子提交相关修改，返回新的 `revision` 与几何检查。
3. 连续编辑使用返回的新 revision；遇到冲突重新读取，不盲目重试旧操作。
4. `read_preview` 查看当前画布 PNG。打开的画布自动同步远端修改并上传对应版本截图；工具最多等待 6 秒，不使用旧版本截图，也没有预览请求队列。

```json
{"action":"apply","project_id":"p","figure_id":"f","expected_revision":3,"operations":[{"op":"set_wrapped_text","id":"label","text":"Complete scientific label"}]}
```

正式版本历史保留，可从 GUI 历史恢复。GUI 中同步的 Agent 修改进入原生撤销历史。GUI 本地编辑静止后自动保存；双方同时编辑时用版本校验拒绝过期写入，保留本地内容并显示冲突。

## 工具组成

- `visualization`：项目、画布、局部读取、批量修改、结构化草图、布局、截图、图片信息、放大图、批注标记读取。
- `visualization_translate`、`visualization_move_text`、`visualization_add_text`、`visualization_reflow_text`：组件与文字的类型化直接编辑。
- `visualization_fit`、`visualization_fit_svg_preview`、`visualization_fit_svg_group_preview`、`visualization_scale_illustration`、`visualization_crop_image`：插图几何直接编辑。SVG 源文件哈希仍用于精确路径编辑，与已删除的候选文档哈希无关。
- `visualization_find_space`：只读空白区域建议。
- `bioart`、`bioicons`：搜索、预览和直接插入/替换素材；BioIcons 还提供已保存 SVG 的配色与路径编辑。
- `figure_evidence`：只读科学来源发现与记录读取。

用 `insert_sketch` 向已有画布追加结构；占用画布的整图替换必须明确传 `replace_existing:true`。保留完整标签、无关对象及逐素材来源。几何检查不代替视觉或科学检查。

不再接受 `proposal_id`、`document_sha256`、`allow_branch` 或候选审阅链接参数。旧候选接口不存在，客户端需改用直接编辑协议。现有数据库中遗留候选表不再被创建或访问；不会在启动时删除旧用户数据。

验证：`cargo test -p visualization -p visualization-assets -p visualization-tools`；GUI：`npm run test:canvas:direct`。直接编辑回归覆盖版本连续性、原子失败、并发冲突、局部文字/几何修改、SVG 来源保留与旧协议拒绝。底层几何算法测试保持独立。
