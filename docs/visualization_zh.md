# 可视化（`visualization_container`）

[English](visualization.md) | [中文](visualization_zh.md)

在隔离的 R/ggplot2 OCI 容器中，将已物化的数据文件和 R 绘图脚本渲染为
PNG。旧的宿主机 `Rscript` `visualization` 节点已移除。

## File-to-File 合约

`visualization_container` 使用标准容器文件数据面。端口 0 是数据 `File`，
端口 1 是用户提供的 R 脚本 `File`。固定容器入口会把数据读取为 `df`、执行
脚本、要求生成赋值给 `p` 的图，并将 `plot.png` 发布为不可变 VFS File artifact。

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
基线。节点绑定已发布的不可变 ACR manifest
`autonomics/visualization@sha256:ee9592b77bc5ea0cebfafafbe39550c377204019f451d7a37e13e4ce2e884f15`；
可通过 `ACR_ENDPOINT` 覆盖 registry host。
