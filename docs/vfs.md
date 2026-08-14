# Virtual File System

Autonomics exposes storage resources through one Unix-style namespace. The runtime mounts local directories, S3-compatible buckets, and Aliyun OSS buckets under `vfs://`; DataFusion then reads them with ordinary paths.

## Mount manifest

The runtime loads `state_dir/vfs.toml`. On first launch it creates that file with a root local mount plus any environment-configured `/data/s3` and `/data/oss` mounts, then loads it.

```toml
[[backend]]
id = "default"
type = "local"
root = "/"

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
source = "/mnt/disk3/test"

[[mount]]
path = "/data/oss"
backend = "oss-prod"
source = "/"
read_only = true
```

`source` explicitly names the file, directory, or object prefix being mounted, analogous to a Linux bind mount. For local backends it can be an absolute host path; it must lie inside the backend root. For S3/OSS it is the prefix inside the configured bucket, with `/` meaning the bucket root.

Paths are routed by the longest matching mount and rewritten from `source` to the virtual `path`. For example:

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
root = "/"

[[mount]]
path = "/data/local/gwas"
backend = "gwas-local"
source = "/mnt/disk2/gwas"
read_only = true
```

DataFusion can then use:

```text
vfs:///data/local/gwas/foo.parquet
```

Credentials remain runtime configuration in `vfs.toml`.

## Current LDSC mounts

The local converted panels are mounted under:

```toml
[[backend]]
id = "ldsc-local"
type = "local"
root = "/"

[[mount]]
path = "/data/ldsc"
backend = "ldsc-local"
source = "/mnt/projects/autonomics_projects/autonomics/reference/ldsc_data/parquet"
read_only = true

[[mount]]
path = "/data/ukbb"
backend = "ldsc-local"
source = "/mnt/disk3/ld_score"
read_only = true
```

The LDSC nodes use these VFS paths:

```text
vfs:///data/ldsc/1000g_eur.parquet
vfs:///data/ldsc/1000g_eur_m.parquet
vfs:///data/ldsc/baselineLD_v2_2_eur.parquet
vfs:///data/ldsc/baselineLD_v2_2_eur_m.parquet
vfs:///data/ukbb/UKBB.EUR.ldscore.parquet/
```
