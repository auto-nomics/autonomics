# Python IO runtime

Dedicated image for the built-in `python_script` implementation. The image
contains Python 3.12 and preinstalled general-purpose IO dependencies.

Published image: `ghcr.io/auto-nomics/autonomics/python-io:0.1.0`

Pinned manifest: `sha256:68872ecf914a961590775b57919c36efda31867d9e2a1a7dc21b7eef1b867fa6`

The published image targets Linux/amd64.

| Area | Packages |
| --- | --- |
| Tables and Arrow | pandas, polars, pyarrow, NumPy |
| Excel | openpyxl, xlsxwriter, xlrd, fastexcel |
| Filesystems and S3 | fsspec, s3fs |
| HTTP | requests, httpx |
| SQL | SQLAlchemy, DuckDB, Python's sqlite3 |
| HTML/XML | lxml, beautifulsoup4, defusedxml |
| JSON/YAML | orjson, ijson, PyYAML, Python's json |
| Compression | zstandard; Python's gzip, bz2, lzma, zipfile and tarfile |

All dependencies, including transitive packages, are version- and hash-locked
in `requirements.txt`. The Python base image is pinned by manifest digest.
There are no runtime package downloads. Network-dependent libraries are
available for environments that allow networking; the existing Python node
continues to use its isolated network profile.

## Build and validate

Run from the repository root (Podman global storage options may be supplied
before `build` and `run` when using a private store):

```bash
podman build --format docker \
  -t ghcr.io/auto-nomics/autonomics/python-io:0.1.0 \
  containers/python-io
podman run --rm --network none --read-only \
  --tmpfs /tmp:rw,nosuid,nodev,size=256m \
  ghcr.io/auto-nomics/autonomics/python-io:0.1.0 \
  python /opt/python-io/smoke.py
```

The smoke program tests pandas/Polars CSV, TSV, Parquet, Arrow IPC, Excel and
JSONL interoperability, gzip, fsspec, SQLite and DuckDB. It also imports every
additional IO dependency. The same program runs during the image build.

Publish the validated image and obtain its registry manifest digest:

```bash
podman push --digestfile /tmp/python-io.digest \
  ghcr.io/auto-nomics/autonomics/python-io:0.1.0
```

`PYTHON_IO_IMAGE_DIGEST` in `crates/node-bundles/nodes-io/src/script_nodes.rs`
must match that digest. The existing `builtin_script_images()` inventory feeds
the CLI's image readiness/preparation checks automatically. This image change
does not register or enable the currently disabled Python node.

## Update dependencies

```bash
uv pip compile containers/python-io/requirements.in --python-version 3.12 \
  --generate-hashes --output-file containers/python-io/requirements.txt \
  --no-emit-index-url --only-binary :all:
```

Rebuild, run the offline smoke program, publish under a new version, and update
the Rust digest together. The node's DataFrame variables remain pandas objects;
scripts can explicitly `import polars as pl` and use paths or pandas/Arrow
conversion to work with Polars.
