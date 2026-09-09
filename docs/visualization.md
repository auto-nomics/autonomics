# Visualization (`visualization_container`)

[English](visualization.md) | [中文](visualization_zh.md)

Renders staged data and R plot files to PNG in an isolated R/ggplot2 OCI
container. The legacy host-`Rscript` `visualization` node has been removed.

## File-to-File contract

`visualization_container` uses the standard container file data plane. Port 0 is
the data File and port 1 is the user's R script File. The node loads the data as
`df`, sources the script, requires a plot assigned to `p`, and publishes
`plot.png` as an immutable VFS File artifact.

Supported `data_format` values are `csv`, `tsv`, `parquet`, `arrow_stream`, and
`arrow_file`. Optional dimensions, resource limits, timeout, and artifact prefix
are controlled by the node spec:

```json
{
  "data_format": "parquet",
  "width": 8,
  "height": 6,
  "dpi": 150
}
```

The node requires Podman access but no host R installation. The container runs
with no network, a read-only root filesystem, and default
limits of 2 CPUs, 2 GiB memory, 256 PIDs, and 300 seconds. Build the local image
with:

```bash
podman build \
  --network host \
  -f containers/visualization/Dockerfile \
  -t localhost/atc/visualization:0.1.0 \
  containers/visualization
```

Run `containers/visualization/test_visualization.sh` for the package-version and
PNG smoke baseline. The node binds the published immutable ACR manifest
`autonomics/visualization@sha256:ee9592b77bc5ea0cebfafafbe39550c377204019f451d7a37e13e4ce2e884f15`;
`ACR_ENDPOINT` may override the registry host.
