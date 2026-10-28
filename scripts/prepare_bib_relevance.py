#!/usr/bin/env python3
"""Download checksum-pinned public BEIR SciFact inputs and normalize JSONL.

Requires pyarrow only for Parquet conversion. Raw data and generated files stay
in the explicitly selected directory; no corpus or model is committed to Git.
"""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import time
import urllib.request

from evaluate_bib_relevance import judgments, sha256

CORPUS_REVISION = "b3b5335604bf5ee3c4447671af975ea25143d4f5"
QRELS_REVISION = "2938d17dc3b09882fdb8c12bbbe2e2dc0e75a029"
SOURCES = {
    "corpus.parquet": (f"https://huggingface.co/datasets/BeIR/scifact/resolve/{CORPUS_REVISION}/corpus/corpus-00000-of-00001.parquet", "243324b35f03d82bd6d98a5f575966876e86cad7ce16e5333a35b1b793dc4f45"),
    "queries.parquet": (f"https://huggingface.co/datasets/BeIR/scifact/resolve/{CORPUS_REVISION}/queries/queries-00000-of-00001.parquet", "1c37956c5dc8b810b60302323c24d1a9e79e26411ba8f5ad9d0888642e2a9034"),
    "train.tsv": (f"https://huggingface.co/datasets/BeIR/scifact-qrels/resolve/{QRELS_REVISION}/train.tsv", "a53f2114831916c096b6c37d9e54da68cef4efdcdbd5ed46533601af972acf1d"),
    "test.tsv": (f"https://huggingface.co/datasets/BeIR/scifact-qrels/resolve/{QRELS_REVISION}/test.tsv", "0864bb985e0ca2367ba217977e72004d549054b2b06666ed9d4825ac7c21284c"),
}


def download(path, url, digest):
    if path.exists():
        if sha256(path) != digest:
            raise ValueError(f"cached source checksum mismatch: {path.name}")
        return
    for attempt in range(3):
        try:
            with urllib.request.urlopen(url, timeout=60) as response:
                content = response.read(16 * 1024 * 1024 + 1)
            if len(content) > 16 * 1024 * 1024 or hashlib.sha256(content).hexdigest() != digest:
                raise ValueError("source exceeds size bound or checksum differs")
            temporary = path.with_suffix(path.suffix + ".part")
            temporary.write_bytes(content)
            temporary.replace(path)
            return
        except OSError:
            if attempt == 2:
                raise
            time.sleep(attempt + 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    root = args.directory
    raw = root / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    for name, (url, digest) in SOURCES.items():
        download(raw / name, url, digest)
    import pyarrow
    import pyarrow.parquet as parquet
    all_queries = []
    for name in ("corpus", "queries"):
        table = parquet.read_table(raw / f"{name}.parquet")
        if table.column_names != ["_id", "title", "text"]:
            raise ValueError("unexpected source schema")
        rows = table.to_pylist()
        if name == "queries":
            all_queries = rows
        with (root / f"{name}.jsonl").open("w", encoding="utf-8") as output:
            for row in rows:
                output.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")
    (root / "qrels").mkdir(exist_ok=True)
    counts = {}
    for split in ("train", "test"):
        source = raw / f"{split}.tsv"
        (root / "qrels" / f"{split}.tsv").write_bytes(source.read_bytes())
        with source.open(encoding="utf-8") as data:
            ids = {row["query-id"] for row in csv.DictReader(data, delimiter="\t")}
        selected = [row for row in all_queries if row["_id"] in ids]
        counts[split] = len(selected)
        with (root / f"queries-{split}.jsonl").open("w", encoding="utf-8") as output:
            for row in selected:
                # Retrieval input carries no relevance grades or relevant document IDs.
                output.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")
    for split in ("train", "test"):
        judgments(root, split)
    files = ["corpus.jsonl", "queries.jsonl", "queries-train.jsonl", "queries-test.jsonl", "qrels/train.tsv", "qrels/test.tsv"]
    manifest = {"schema": "bib-relevance-dataset-v1", "dataset": "BEIR SciFact", "language": "en",
                "corpus_revision": CORPUS_REVISION, "qrels_revision": QRELS_REVISION,
                "declared_license": "cc-by-sa-4.0 (Hugging Face dataset card)",
                "dataset_card": "https://huggingface.co/datasets/BeIR/scifact",
                "original_dataset": "https://github.com/allenai/scifact",
                "splits": counts, "tuning_split": "train", "acceptance_split": "test",
                "limitations": ["scientific claim retrieval, not known-ID, GUI, or full-text evidence evaluation",
                                "English only; no Chinese relevance judgments implied",
                                "published qrels are incomplete judgments, not exhaustive relevance",
                                "official split independence does not imply absence from model pretraining"],
                "sources": {name: {"url": url, "sha256": digest} for name, (url, digest) in SOURCES.items()},
                "generated_sha256": {name: sha256(root / name) for name in files},
                "conversion": {"pyarrow_version": pyarrow.__version__, "json": "UTF-8, sorted keys, source row order"}}
    (root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(manifest, ensure_ascii=False))


if __name__ == "__main__":
    main()
