#!/usr/bin/env sh
# End-to-end smoke test for the pathology container on the CPU image.
# Generates a synthetic pyramid WSI + IHC slide + label masks + a tiny
# random-weight timm checkpoint, then exercises all seven runner commands
# and checks their outputs.
set -eu

IMAGE=${AUTONOMICS_PATHOLOGY_IMAGE:-localhost/pathology:cpu}

if command -v podman >/dev/null 2>&1; then
  CONTAINER_CLI=podman
else
  CONTAINER_CLI=docker
fi

WORK=$(mktemp -d /tmp/autonomics-pathology-smoke.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

cat > "$WORK/gen_fixture.py" <<'PY'
import numpy as np
import tifffile
from PIL import Image

rng = np.random.default_rng(11)
W, H = 4096, 3072


def noisy(fill, shape):
    return np.clip(fill + rng.integers(-10, 10, shape), 0, 255).astype(np.uint8)


def blob_mask(centers, radii):
    yy, xx = np.mgrid[0:H, 0:W]
    mask = np.zeros((H, W), bool)
    for (cx, cy), r in zip(centers, radii):
        mask |= ((xx - cx) ** 2 + (yy - cy) ** 2) <= r**2
    return mask


# H&E-like slide: near-white background, pink stroma blob, purple tumor blob.
slide = np.full((H, W, 3), 238, np.uint8)
stroma = blob_mask([(1400, 1000), (2600, 2100)], [520, 380])
tumor = blob_mask([(2200, 1500)], [640])
slide[stroma] = noisy(np.array([196, 138, 168]), slide[stroma].shape)
slide[tumor] = noisy(np.array([136, 84, 158]), slide[tumor].shape)

levels = [slide]
for factor in (4, 16):
    small = Image.fromarray(levels[0]).resize(
        (W // factor, H // factor), Image.BILINEAR
    )
    levels.append(np.asarray(small))

with tifffile.TiffWriter("/work/wsi.tif") as store:
    store.write(
        levels[0],
        photometric="rgb",
        planarconfig="contig",
        resolution=(10000, 10000),
        resolutionunit="CENTIMETER",
        subifds=2,
    )
    for level in levels[1:]:
        store.write(level, subfiletype=1, photometric="rgb", planarconfig="contig")

# IHC-like slide: light background with weak and strong DAB-brown blobs.
ihc = np.full((H, W, 3), 232, np.uint8)
weak = blob_mask([(1200, 900), (2900, 2000)], [420, 360])
strong = blob_mask([(2100, 1600)], [480])
ihc[weak] = noisy(np.array([172, 122, 84]), ihc[weak].shape)
ihc[strong] = noisy(np.array([110, 66, 34]), ihc[strong].shape)
tifffile.imwrite(
    "/work/ihc.tif",
    ihc,
    photometric="rgb",
    planarconfig="contig",
    resolution=(10000, 10000),
    resolutionunit="CENTIMETER",
)

# ROI mask (label 1) and annotation mask (labels 1+2) at downsample 16.
# Label masks store label indices directly; the runner reads raw pixel values.
mh, mw = H // 16, W // 16
roi = np.zeros((mh, mw), np.uint8)
roi[40:120, 60:180] = 1
Image.fromarray(roi).save("/work/roi_mask.png")

anno = np.zeros((mh, mw), np.uint8)
anno[30:100, 40:150] = 1
yy, xx = np.mgrid[0:mh, 0:mw]
anno[((xx - 190) ** 2 + (yy - 110) ** 2 <= 28**2) & (anno == 0)] = 2
Image.fromarray(anno).save("/work/anno_mask.png")
print("fixtures written")
PY

cat > "$WORK/gen_model.py" <<'PY'
import json
import timm
import torch

# Must mirror the runner's default timm kwargs (init_values, dynamic_img_size)
# so the strict checkpoint load in wsi-embed sees matching parameters.
model = timm.create_model(
    "vit_tiny_patch16_224",
    pretrained=False,
    num_classes=0,
    img_size=224,
    init_values=1e-5,
    dynamic_img_size=True,
)
torch.save(model.state_dict(), "/work/model/random_vit_tiny.pth")
json.dump(
    {"arch": "vit_tiny_patch16_224", "img_size": 224, "timm_kwargs": {}},
    open("/work/model/model_config.json", "w"),
)
print("random checkpoint written")
PY

cat > "$WORK/check.py" <<'PY'
import json
import sys

import h5py
import numpy as np
import pandas as pd

what = sys.argv[1]


def load(path):
    return json.load(open(path))


if what == "ingest":
    meta = load("/work/slide_meta.json")
    assert meta["slide"]["dimensions"] == [4096, 3072], meta["slide"]["dimensions"]
    assert meta["slide"]["level_count"] == 3, meta["slide"]["level_count"]
    assert meta["slide"]["mpp_known"], meta["slide"]
    assert abs(meta["slide"]["mpp_x"] - 1.0) < 1e-6, meta["slide"]["mpp_x"]
elif what == "qc":
    summary = load("/work/qc_summary.json")
    assert summary["n_tiles"] > 0
    assert summary["mean_tissue_fraction"] > 0.02, summary
    tiles = pd.read_parquet("/work/tile_qc.parquet")
    assert {"tissue_fraction", "focus_laplacian_variance"} <= set(tiles.columns)
elif what == "patches":
    meta = load("/work/patch_meta.json")
    assert meta["n_selected"] > 0, meta
    frame = pd.read_parquet("/work/patches.parquet")
    assert len(frame) == meta["n_selected"]
    assert list(frame.columns) == meta["columns"], list(frame.columns)
    assert (frame["tissue_fraction"] >= meta["min_tissue_fraction"]).all()
    assert frame["x"].max() < 4096 and frame["y"].max() < 3072
elif what == "embed":
    meta = load("/work/embed_meta.json")
    assert meta["device"] == "cpu", meta["device"]
    assert meta["model"]["arch"] == "vit_tiny_patch16_224"
    assert meta["model"]["checkpoint_sha256"].startswith("sha256:")
    with h5py.File("/work/embeddings.h5", "r") as handle:
        assert handle["embeddings"].shape == (meta["n_patches"], 192), handle[
            "embeddings"
        ].shape
        assert handle["embeddings"].dtype == np.float32
        ids = [value.decode() for value in handle["patch_ids"][()]]
    assert len(ids) == meta["n_patches"]
elif what == "domain":
    meta = load("/work/domain_meta.json")
    # Same-slide resamples are near-exchangeable: silhouette may sit near 0
    # (its full range on cosine distance is [-1, 1]).
    assert -1.0 <= meta["batch_silhouette"] <= 1.0, meta
    assert meta["embedding_dim"] == 192, meta
    assert len(meta["top_shifted_dimensions"]) == 10
    row = pd.read_parquet("/work/domain_metrics.parquet")
    assert len(row) == 1
    assert row["batch_silhouette"].iloc[0] == meta["batch_silhouette"]
elif what == "ihc":
    summary = load("/work/ihc_summary.json")
    assert summary["n_tiles"] > 0
    assert summary["positive_fraction"] > 0.05, summary
    assert 0.0 <= summary["h_score"] <= 300.0, summary
    tiles = pd.read_parquet("/work/tile_ihc.parquet")
    assert {"positive_fraction", "mean_dab"} <= set(tiles.columns)
elif what == "qupath":
    meta = load("/work/geojson_meta.json")
    collection = load("/work/annotations.geojson")
    assert collection["type"] == "FeatureCollection"
    assert len(collection["features"]) == meta["n_features"] >= 2, meta
    names = {
        feature["properties"]["classification"]["name"]
        for feature in collection["features"]
    }
    assert names == {"Tumor", "label_2"}, names
    first = collection["features"][0]["geometry"]["coordinates"][0]
    assert first[0] == first[-1], "polygon ring must be closed"
    assert max(x for x, _ in first) <= 4096
print(f"check {what}: ok")
PY

echo "== build fixtures =="
mkdir -p "$WORK/model"
"$CONTAINER_CLI" run --rm -v "$WORK:/work" --entrypoint python "$IMAGE" /work/gen_fixture.py
"$CONTAINER_CLI" run --rm -v "$WORK:/work" --entrypoint python "$IMAGE" /work/gen_model.py

run_cmd() {
  "$CONTAINER_CLI" run --rm -v "$WORK:/work" \
    -e "AUTONOMICS_INPUT0=$1" -e "AUTONOMICS_INPUT1=$2" -e "AUTONOMICS_INPUT2=$3" \
    -e "AUTONOMICS_OUTPUT0=$4" -e "AUTONOMICS_OUTPUT1=$5" -e "AUTONOMICS_OUTPUT2=$6" \
    -e "$7" \
    "$IMAGE" "$8"
}

echo "== wsi-ingest =="
run_cmd /work/wsi.tif "" "" /work/thumbnail.png /work/slide_meta.json "" \
  "PATHOLOGY_WSI_SETTINGS={}" wsi-ingest
"$CONTAINER_CLI" run --rm -v "$WORK:/work" --entrypoint python "$IMAGE" /work/check.py ingest

echo "== wsi-qc =="
run_cmd /work/wsi.tif "" "" /work/tile_qc.parquet /work/qc_summary.json "" \
  "PATHOLOGY_QC_SETTINGS={\"tile_size\":128,\"max_tiles\":50}" wsi-qc
"$CONTAINER_CLI" run --rm -v "$WORK:/work" --entrypoint python "$IMAGE" /work/check.py qc

echo "== patch-sample (two seeds for the domain check) =="
run_cmd /work/wsi.tif "" "" /work/patches_a.parquet /work/tissue_mask.png /work/patch_meta.json \
  "PATHOLOGY_PATCH_SETTINGS={\"patch_size\":256,\"max_patches\":8,\"min_tissue_fraction\":0.4,\"seed\":3}" patch-sample
mv "$WORK/patch_meta.json" "$WORK/patch_meta_a.json"
run_cmd /work/wsi.tif "" "" /work/patches_b.parquet /work/tissue_mask.png /work/patch_meta.json \
  "PATHOLOGY_PATCH_SETTINGS={\"patch_size\":256,\"max_patches\":8,\"min_tissue_fraction\":0.4,\"seed\":4}" patch-sample
cp "$WORK/patches_b.parquet" "$WORK/patches.parquet"
"$CONTAINER_CLI" run --rm -v "$WORK:/work" --entrypoint python "$IMAGE" /work/check.py patches

echo "== wsi-embed (random vit-tiny, CPU) =="
for side in a b; do
  run_cmd /work/wsi.tif "/work/patches_${side}.parquet" "/work/model/random_vit_tiny.pth,/work/model/model_config.json" \
    /work/embeddings_${side}.h5 /work/embed_meta.json "" \
    "PATHOLOGY_EMBED_SETTINGS={\"device\":\"cpu\",\"batch_size\":4}" wsi-embed
  mv "$WORK/embed_meta.json" "$WORK/embed_meta_${side}.json"
done
cp "$WORK/embeddings_a.h5" "$WORK/embeddings.h5"
cp "$WORK/embed_meta_a.json" "$WORK/embed_meta.json"
"$CONTAINER_CLI" run --rm -v "$WORK:/work" --entrypoint python "$IMAGE" /work/check.py embed

echo "== domain-check =="
run_cmd /work/embeddings_a.h5 /work/embeddings_b.h5 "" /work/domain_metrics.parquet /work/domain_meta.json "" \
  "PATHOLOGY_DOMAIN_SETTINGS={}" domain-check
"$CONTAINER_CLI" run --rm -v "$WORK:/work" --entrypoint python "$IMAGE" /work/check.py domain

echo "== ihc-quant =="
run_cmd /work/ihc.tif /work/roi_mask.png "" /work/tile_ihc.parquet /work/ihc_summary.json "" \
  "PATHOLOGY_IHC_SETTINGS={\"roi_label\":1,\"mask_downsample\":16,\"tile_size\":256}" ihc-quant
"$CONTAINER_CLI" run --rm -v "$WORK:/work" --entrypoint python "$IMAGE" /work/check.py ihc

echo "== qupath-import =="
run_cmd /work/anno_mask.png /work/wsi.tif "" /work/annotations.geojson /work/geojson_meta.json "" \
  "PATHOLOGY_QUPATH_SETTINGS={\"label_names\":{\"1\":\"Tumor\"},\"mask_downsample\":16}" qupath-import
"$CONTAINER_CLI" run --rm -v "$WORK:/work" --entrypoint python "$IMAGE" /work/check.py qupath

echo "PATHOLOGY SMOKE OK"
