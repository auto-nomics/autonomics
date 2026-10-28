#!/usr/bin/env python3
"""Compare projected vector reads on copies of a closed synthetic fixture only.

This is a storage diagnostic, not complete snapshot/search/inference latency.
Build semantic_storage_probe with --release before running this script.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True,
                        help="Closed successful synthetic run's count directory")
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    if not hasattr(os, "posix_fadvise"):
        parser.error("POSIX file-cache advice is required by this diagnostic protocol")
    previous = args.source / "receipt.json"
    source_receipt = json.loads(previous.read_text(encoding="utf-8"))
    if source_receipt.get("controlled_http_model_fixture") is not True:
        parser.error("source must be a completed synthetic service fixture")
    files = sorted(args.source.glob("library.db.semantic-v1*"))
    if not (args.source / "library.db.semantic-v1").is_file() or any(
        not path.is_file() or path.is_symlink() for path in files
    ):
        parser.error("regular non-symlink synthetic sidecar files required")
    args.directory.mkdir(exist_ok=False)
    worker = args.worker.resolve()
    receipt = {"schema": "bib-vector-storage-comparison-v2",
               "source_receipt_sha256": sha256(previous),
               "worker_sha256": sha256(worker),
               "source_sha256": {str(p): sha256(p) for p in [
                   Path(__file__), Path("crates/bib-base/examples/semantic_storage_probe.rs")]},
               "copied_fixture_only": True, "physical_cache_guaranteed": False,
               "cache_advice": "fsync_then_posix_fadvise_DONTNEED", "runs": {}}
    fingerprint = None
    for mode in ["join", "scan", "rowid"]:
        root = args.directory / mode
        root.mkdir()
        for source in files:
            path = root / source.name
            shutil.copy2(source, path)
            with path.open("rb") as file:
                os.fsync(file.fileno())
                os.posix_fadvise(file.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
        result = subprocess.run([str(worker), str(root / "library.db.semantic-v1"), mode],
                                capture_output=True, text=True, timeout=180, check=False)
        (root / "stdout.log").write_text(result.stdout, encoding="utf-8")
        (root / "stderr.log").write_text(result.stderr, encoding="utf-8")
        result.check_returncode()
        run = json.loads(result.stdout)
        if run.get("schema") != "bib-vector-storage-diagnostic-v1":
            raise ValueError("unexpected worker receipt")
        if fingerprint is not None and run["content_fingerprint"] != fingerprint:
            raise ValueError("storage strategies returned different referenced vectors")
        fingerprint = run["content_fingerprint"]
        receipt["runs"][mode] = run
        (args.directory / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n",
                                                    encoding="utf-8")
        print(json.dumps({"mode": mode, "read_ms": run["read_ms"],
                          "unique_vectors": run["unique_vectors"]}), flush=True)


if __name__ == "__main__":
    main()
