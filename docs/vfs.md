# Virtual File System

Autonomics exposes storage resources through one Unix-style namespace. The runtime mounts local directories, S3-compatible buckets, and Aliyun OSS buckets under `vfs://`; DataFusion then reads them with ordinary paths.

## Mount manifest

The runtime loads `state_dir/vfs.toml`. If it does not exist, a root local mount plus environment-configured `/data/s3` and `/data/oss` mounts are created.

```toml
[[backend]]
id = "default"
type = "local"
root = "/mnt/disk3/test"

[[backend]]
id = "oss-prod"
type = "oss"
bucket = "autonomics-data"
endpoint = "https://oss-cn-hangzhou.aliyuncs.com"
access_key_id = "..."
secret_access_key = "..."

[[mount]]
path = "/"
backend = "default"

[[mount]]
path = "/data/oss"
backend = "oss-prod"
read_only = true
```

Paths are routed by the longest matching mount and rewritten to the backend key. For example:

```text
vfs:///data/oss/ld_score/1000g_eur/part.parquet
```

is read from:

```text
oss://autonomics-data/ld_score/1000g_eur/part.parquet
```

A local parquet can be exposed the same way by mounting its containing directory:

```toml
[[backend]]
id = "gwas-local"
type = "local"
root = "/mnt/disk2/gwas"

[[mount]]
path = "/data/local/gwas"
backend = "gwas-local"
read_only = true
```

DataFusion can then use:

```text
vfs:///data/local/gwas/foo.parquet
```

Credentials remain runtime configuration in `vfs.toml`.
