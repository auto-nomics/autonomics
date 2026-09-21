# 可视化（`visualization_container`）

[English](visualization.md) | [中文](visualization_zh.md)

在隔离的 R/ggplot2 OCI 容器中，将已经完成全部计算、可直接绘图的数据文件和
受限 R 绘图脚本渲染为 PNG。`visualization_container` 是终端 sink，不能接入
下游 DAG 节点。旧的宿主机 `Rscript` `visualization` 节点已移除。

## File-to-File 合约

`visualization_container` 使用标准容器文件数据面。端口 0 是数据 `File`，
端口 1 是用户提供的 R 脚本 `File`。脚本会在容器启动前校验，只允许如下形态的
单个受限表达式：

```r
p <- ggplot2::ggplot(df, ggplot2::aes(x, y)) + ggplot2::geom_point()
```

只允许调用白名单中非计算性的 `ggplot2::` 绘图函数。注释、控制流、任意 R
函数、索引、文件 I/O、直方图/平滑曲线等会计算的 geom 或 stat、在 aesthetic
中执行计算、超过 64 KiB 的脚本，以及从渲染图继续连下游边都会被拒绝。过滤、
聚合、归一化、建模和所有其他数据转换必须发生在上游节点。固定容器入口会把
数据读取为 `df`、要求生成赋值给 `p` 的图，并将
`plot.png` 发布为不可变 VFS File artifact。

`data_format` 支持 `csv`、`tsv`、`parquet`、`arrow_stream` 和 `arrow_file`。
尺寸、资源限制、超时和 artifact 前缀可通过节点 spec 控制：

```json
{
  "data_format": "parquet",
  "width": 8,
  "height": 6,
  "dpi": 150
}
```

节点需要可用的 Podman，但不需要宿主机 R。容器默认无网络、root filesystem
只读，并使用 2 CPU、2 GiB 内存、256 PID、300 秒的默认限制。构建本地镜像：

```bash
podman build \
  --network host \
  -f containers/visualization/Dockerfile \
  -t localhost/atc/visualization:0.1.0 \
  containers/visualization
```

运行 `containers/visualization/test_visualization.sh` 可执行包版本与 PNG 冒烟
基线。节点绑定已发布的不可变 GHCR manifest
`autonomics/visualization@sha256:ee9592b77bc5ea0cebfafafbe39550c377204019f451d7a37e13e4ce2e884f15`；
可通过 `AUTONOMICS_IMAGE_PREFIX` 覆盖 registry namespace。
