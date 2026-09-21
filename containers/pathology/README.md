# Pathology container

Whole-slide-image Stage-A/B tooling for the chordoma protocol: ingestion, QC,
deterministic patch sampling, foundation-model embedding, batch-domain checks,
IHC quantification, and QuPath annotation export. One runner
(`pathology_runner.py`), two image variants:

- `Dockerfile` — CPU variant (torch 2.3.1 CPU wheel on the digest-pinned
  python:3.11.11-slim base). Built and smoke-tested locally.
- `Dockerfile.cuda` — CUDA variant (`pytorch/pytorch:2.3.1-cuda12.1-cudnn8-runtime`
  base) for production GPU hosts. Rootless Podman needs the
  nvidia-container-toolkit CDI spec (`nvidia-ctk cdi generate`), and the node
  must request `gpus` (the `pathology_wsi_embed` node defaults to `all`).

## Commands

| Command | Inputs | Outputs |
| --- | --- | --- |
| `wsi-ingest` | 0: WSI | thumbnail.png, slide_meta.json |
| `wsi-qc` | 0: WSI | tile_qc.parquet, qc_summary.json |
| `patch-sample` | 0: WSI | patches.parquet, tissue_mask.png, patch_meta.json |
| `wsi-embed` | 0: WSI, 1: patch table, 2: model bundle | embeddings.h5, embed_meta.json |
| `domain-check` | 0: reference h5, 1: comparison h5 | domain_metrics.parquet, domain_meta.json |
| `ihc-quant` | 0: WSI, 1: ROI mask image | tile_ihc.parquet, ihc_summary.json |
| `qupath-import` | 0: label mask, 1: reference WSI | annotations.geojson, geojson_meta.json |

Paths follow the standard container contract (`AUTONOMICS_INPUT{n}`,
`AUTONOMICS_OUTPUT{n}`); every command reads its knobs from the matching
`PATHOLOGY_*_SETTINGS` JSON environment variable whose defaults mirror the
Rust spec defaults in
`crates/node-bundles/nodes-io/src/pathology_container.rs`.

Patch coordinates are level-0 top-left pixels with `read_region` semantics;
the same seed always yields the same patch set on the same slide.

## Model weights are never downloaded

`wsi-embed` takes the model as input port 2 (a FileSet): an optional
`model_config.json` (`{"arch": ..., "img_size": ..., "timm_kwargs": {...}}`)
plus exactly one `.pth`/`.pt`/`.safetensors` checkpoint. The default config
targets UNI-style `vit_large_patch16_224` with `init_values=1e-5`.

UNI (and other foundation-model checkpoints like CONCH, Virchow, GigaPath)
are **license-gated on Hugging Face under research-only terms**. The runner
contains no download path by design. The operator must:

1. accept the model license on the Hugging Face model page,
2. download the checkpoint with an authenticated account,
3. stage it into the DAG as an input FileSet (or a catalog panel).

Every embedding h5 records the checkpoint's sha256 in its attributes and in
`embed_meta.json`, so provenance survives the license boundary.

## Local build and smoke test

```bash
podman build -t localhost/pathology:cpu -f Dockerfile .
./test_pathology_smoke.sh   # synthetic-slide end-to-end run of all commands
```

The smoke test generates a synthetic pyramid TIFF with tissue-like blobs,
then exercises all seven commands on the CPU image. For `wsi-embed` it uses
a tiny random-weight `vit_tiny_patch16_224` checkpoint so no gated download
is needed; the production UNI checkpoint follows the staging flow above.

## Publish

```bash
podman tag localhost/pathology:cpu ghcr.io/auto-nomics/autonomics/pathology:<tag>
podman push ghcr.io/auto-nomics/autonomics/pathology:<tag>
# then replace PATHOLOGY_IMAGE_DIGEST in pathology_container.rs with the
# post-push manifest digest and update this section:
#
# Current digest: sha256:a0edcb6cca25f009f669723406207651284960425f7255891be5b91b29b63f2f (cpu-r1)
```
