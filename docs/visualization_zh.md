# 可视化 (`visualization`)

[English](visualization.md) | [中文](visualization_zh.md)

通过 R/ggplot2 将 DataFusion `DataFrame` 渲染为 PNG 的 DAG 节点。

## R/ggplot2 渲染

数据以 **Arrow IPC 流**（`arrow::ipc::writer::StreamWriter` → `arrow::read_ipc_stream`）跨 Rust→R 边界，列类型被精确保留——无需 CSV 重新推断，无需逐行 JSON。

```text
DataFrame → collect() → Arrow IPC 流字节 → 临时目录 → Rscript
   (arrow::read_ipc_stream → df → <r_code> → ggsave) → PNG 字节
   → opendal op.write(虚拟路径) → NodeReport.artifact_path
```

## 子进程，而非进程内 R

渲染器调用 `Rscript` 而非在进程内链接 `libR`。因此 R 是_仅运行时、可选_的依赖：工作空间可在未安装 R 的情况下编译，核心流水线（LDSC、MR 等）也可在无 R 时运行。缺失或配置错误的 R 在渲染时以类型化的 `VizError::RscriptNotFound` / `RscriptFailed` 呈现，绝不会导致构建失败。

## DAG 节点

作为 `visualization` 节点类型暴露（`data-engine/nodes/viz.rs`），智能体通过通用的 `add_node` / `run_dag` 工具访问——无需专用工具。它镜像 `SinkNode`：一个无类型输入端口，无输出端口。渲染路径通过 `NodeReport.artifact_path` 报告回。

## opendal 输出

PNG 写入引擎的 **opendal 虚拟化文件系统**（与源/汇数据相同的隔离空间），而非宿主文件系统。`output_path` 是虚拟路径（如 `/plots/scatter.png`）；opendal 句柄从构建器贯穿 `NodeCtx.opendal`。

## 绘图规范

`r_code` 字段是 ggplot2 R 代码，运行时已有一个名为 `df` 的 `data.frame` 绑定到输入；它必须构建一个图并将其赋值给名为 `p` 的变量。

示例：

```r
p <- ggplot(df, aes(x = bp, y = pval)) + geom_point()
```

尺寸（`width`/`height`/`dpi`）为可选项。

## R 依赖（可选）

渲染需要 `PATH` 上的 `Rscript`，并安装 `arrow` 和 `ggplot2` 包。可通过 `VISUALIZATION_RSCRIPT` 环境变量覆盖二进制路径。引擎**不**检测或锁定 R 版本——启动进程解析到的 `Rscript` 即生效。conda 环境是提供已知良好 R 的最简洁方式（预期包见该 crate 的测试）。
