# Plugin Authoring Guide

This guide explains how to create a new plugin from scratch. It is written
for tool authors who already know what analysis they want to expose but need
to map it to Autonomics' container-node contract.

For field-by-field schema rules, see the [manifest reference](manifest-reference.md).
For release gates, see [testing and release](testing-and-release.md).

## 1. Define the Node Boundary

Start with the scientific contract, not with command-line flags:

- What exact table or file enters the node?
- What exact file leaves it?
- Which identifiers, columns, delimiters, missing values, and duplicate rules
  are meaningful?
- Which parameters genuinely belong in a DAG spec?
- Which implementation details should remain fixed in the image or script?

Prefer a small, predictable node over a generic wrapper around every upstream
option. Fix mature defaults in the manifest and expose only parameters an
analyst needs to change.

Multiple related invocations can share one plugin only when they use the same
image and panel bindings. If they need different images or panels, create
separate plugins.

## 2. Scaffold the Repository

Create a lowercase kebab-case directory under your plugin checkout root, for
example `/mnt/projects/node-plugins/clusterprofiler`:

```bash
cd /path/to/autonomics
cargo run -p container-plugin --bin nodedev -- \
  init --name clusterprofiler --dir /mnt/projects/node-plugins
```

`nodedev init` only creates a manifest skeleton with a placeholder image. The
remaining layout is yours to complete:

```text
clusterprofiler/
├── manifest.toml
├── scripts/
│   └── clusterprofiler_ora.R.sh
├── Dockerfile
├── README.md
├── test_clusterprofiler_ora.sh
└── fixtures/
    ├── gene_table.tsv
    └── pathways.gmt
```

Initialize the Git repository before making the first reviewable change:

```bash
cd /mnt/projects/node-plugins/clusterprofiler
git init -b main
```

## 3. Draft the Manifest Contract

Declare the image, ports, parameters, command, and resources. The following
fragment shows a useful starting shape for a Bioconductor-style node:

```toml
schema_version = 1
plugin_name = "clusterprofiler"

[image]
reference = "ghcr.io/auto-nomics/autonomics/clusterprofiler@sha256:<digest>"
tag = "0.1.0"
upstream = "Bioconductor clusterProfiler"
license = "Artistic-2.0"

[[nodes]]
kind = "clusterprofiler_ora"
desc = "Runs clusterProfiler over-representation analysis."
doc = """
Input 0 is a TSV gene table. Input 1 is a GMT gene-set file. The node writes
an enrichment TSV and a JSON run report.
"""
timeout_secs = 3600

[nodes.ports]
inputs = [
  { type = "file", label = "gene_table", accepted_formats = ["tsv"] },
  { type = "file", label = "gene_sets", accepted_formats = ["gmt"] },
]
outputs = [
  { path = "clusterprofiler_ora.tsv", format = "clusterprofiler_ora_tsv" },
  { path = "clusterprofiler_ora.json", format = "clusterprofiler_ora_json" },
]

[nodes.params]
gene_col = { type = "string", default = "gene", doc = "Gene identifier column" }
pvalue_cutoff = { type = "number", default = 0.05, min = 0.0, max = 1.0 }

[nodes.command]
interpreter = "Rscript"
script_file = "scripts/clusterprofiler_ora.R.sh"

[nodes.command.env]
AUTONOMICS_GENE_COL = "{{ gene_col }}"
AUTONOMICS_PVALUE_CUTOFF = "{{ pvalue_cutoff }}"
```

While developing, use a temporary valid digest or a previously published
digest only to exercise parsing. Never release a manifest containing an image
you have not built and published. Replace the placeholder with the pushed
image's manifest digest before review.

## 4. Write the Execution Script

The script is the adapter between the node contract and the tool. It must be
deterministic, validate its inputs early, and produce every declared output.

Use the runtime environment instead of interpolating user values into code:

```r
input_path <- Sys.getenv("AUTONOMICS_INPUT0")
gene_col <- Sys.getenv("AUTONOMICS_GENE_COL")
output_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
```

Recommended script structure:

1. Read every parameter and path from environment variables.
2. Fail fast on empty strings, invalid enums, and contradictory bounds that
   the manifest DSL cannot express.
3. Validate input existence, columns, identifiers, numeric values, duplicates,
   and file formats.
4. Normalize only where the scientific contract explicitly permits it.
5. Invoke the pinned tool.
6. Write stable output schemas.
7. Handle biologically valid empty results by writing the promised empty table
   or report rather than failing.

Shell scripts should start with strict behavior:

```sh
#!/bin/sh
set -eu
```

R scripts should avoid implicit type coercion and should explicitly stop with
actionable errors. Python scripts should parse paths and numeric values
explicitly and should not execute untrusted input.

## 5. Build the Image

The Dockerfile is part of the plugin's provenance, not a local convenience.
Use a digest-pinned base and install exact runtime dependencies. For example:

```dockerfile
FROM docker.io/bioconductor/bioconductor:3.21-R-4.5.2@sha256:<digest>

RUN Rscript -e ' \
  BiocManager::install(
    c("clusterProfiler", "data.table", "jsonlite"),
    ask = FALSE,
    update = FALSE
  ) \
'

RUN Rscript -e ' \
  stopifnot(requireNamespace("clusterProfiler", quietly = TRUE)); \
  stopifnot(requireNamespace("data.table", quietly = TRUE)); \
  stopifnot(requireNamespace("jsonlite", quietly = TRUE)) \
'

USER 1001
ENV HOME=/tmp
ENTRYPOINT ["Rscript"]
```

The image must run without writing to its root filesystem. Keep caches,
temporary files, and outputs under `/work` or another runtime-provided writable
mount. If a tool requires `HOME`, point it at `/tmp` rather than relaxing the
read-only rootfs.

Record package versions and upstream sources in the plugin README. For
strict reproducibility, use a package manager snapshot or vendored source;
the final plugin image digest is the executable lock.

## 6. Add Fixtures and a Smoke Test

Small fixtures make script behavior reviewable without a full DAG. At minimum,
test:

- The happy path.
- A missing required column.
- A malformed input.
- A valid but empty result, where applicable.
- Every optional-flag branch.

The smoke test should run the same script content that the loader inlines. For
a Podman-first test, mount fixtures into `/work`, set the same
`AUTONOMICS_INPUT*`, `AUTONOMICS_OUTPUT*`, and parameter variables, and assert
the output files and schemas.

## 7. Add Manifest Tests in Autonomics

New plugins should have a focused integration test under
`crates/container-plugin/tests/`. The test should locate the plugin checkout,
parse and inline its manifest, and assert:

- The image is digest-pinned.
- The expected node kinds are registered.
- Input and output paths match the documented contract.
- Command interpreter and script markers are compiled as expected.
- Environment templates contain all intended parameters.
- Resource defaults or explicit overrides are correct.

The test may skip only when the optional plugin checkout is absent. It must
not skip merely because parsing fails.

## 8. Develop Through a Path Source

Add the local checkout to `~/.autonomics/plugins.toml`:

```toml
[[plugin]]
name = "clusterprofiler"
path = "/mnt/projects/node-plugins/clusterprofiler"
```

Restart the daemon after each manifest or script edit. A symlink does not
bypass validation; it only avoids reinstalling unchanged source during rapid
development.

## 9. Publish the Family

Publication is an explicit operation:

1. Build the image with Podman.
2. Run the plugin smoke tests and focused Autonomics tests.
3. Push a versioned image tag to GHCR.
4. Copy the pushed image's repository digest into `manifest.toml`.
5. Commit the complete plugin directory.
6. Push the plugin Git repository.
7. Reference the exact commit SHA in `~/.autonomics/plugins.toml`.

Never use a local image ID as `image.reference`. Runtime nodes pull and execute
the repository manifest digest, not the local config-layer ID.

## 10. Document the Plugin

Every plugin README should explain:

- Scientific purpose and exact input/output schemas.
- Parameter semantics and validation rules.
- Upstream tool and package versions.
- Image build and publication provenance.
- Data panel or annotation assumptions.
- Known differences from the upstream CLI or library API.
- Test commands and fixture coverage.
- Why any security/resource override is necessary.

The README is part of the plugin contract. Reviewers should reject an opaque
wrapper even when its manifest parses.
