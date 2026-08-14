# Autonomics Infrastructure

This directory contains local reference-data preparation utilities.

Current contents:

- `thousand_genomes/`: helpers for downloading and preprocessing public 1000
  Genomes data.

Dataset persistence is handled by the VFS mounts configured in
`$AUTONOMICS_STATE_DIR/vfs.toml`; project documentation describes those mounts.
