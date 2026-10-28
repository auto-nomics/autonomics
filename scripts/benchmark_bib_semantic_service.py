#!/usr/bin/env python3
"""Run service/actor capacity against a bounded loopback model fixture.

Build semantic_service_benchmark with --release first. This measures the real
service, actor, persistence and ranking path, excluding actual model inference.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import threading
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--evict-file-cache", action="store_true", help="fsync then request DONTNEED only for copied fixture files; not physical-cache proof")
    parser.add_argument("--reuse-from", type=Path, help="Copy a closed synthetic fixture from a previous successful runner directory")
    parser.add_argument("--counts", type=int, nargs="+", default=[10000, 100000])
    args = parser.parse_args()
    if any(n < 100 or n % 100 for n in args.counts):
        parser.error("counts must be positive multiples of 100")
    if args.evict_file_cache and (not args.reuse_from or not hasattr(os, "posix_fadvise")):
        parser.error("file-cache advice requires --reuse-from and POSIX fadvise support")
    worker = args.worker.resolve()
    args.directory.mkdir(exist_ok=False)
    counters = {"embed_requests": 0, "embedded_inputs": 0, "tag_requests": 0}
    lock = threading.Lock()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def reply(self, body):
            data = json.dumps(body, separators=(",", ":")).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            assert self.path == "/api/tags"
            with lock:
                counters["tag_requests"] += 1
            self.reply({"models": [{"name": "autonomics-embedding:" + "a" * 64, "digest": "a" * 64}]})

        def do_POST(self):
            assert self.path == "/api/embed"
            length = int(self.headers["Content-Length"])
            assert 0 < length <= 256_000
            body = json.loads(self.rfile.read(length))
            assert body["truncate"] is False
            inputs = body["input"]
            assert isinstance(inputs, list) and 1 <= len(inputs) <= 16
            vectors = []
            for text in inputs:
                # Deliberately cheap synthetic vectors; integer JSON limits
                # fixture encoding overhead. Rust validates/normalizes them.
                digest = hashlib.sha256(text.encode()).digest()
                vectors.append([value - 128 for value in digest] * 32)
            with lock:
                counters["embed_requests"] += 1
                counters["embedded_inputs"] += len(inputs)
            self.reply({"embeddings": vectors})

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    paths = ["Cargo.lock", "crates/api-server/Cargo.toml", "crates/bib-base/examples/semantic_service_benchmark.rs",
             "scripts/benchmark_bib_semantic_service.py",
             "crates/bib-base/src/semantic_index.rs", "crates/bib-base/src/hybrid_search.rs",
             "crates/bib-base/src/semantic_service.rs", "crates/bib-base/src/search.rs",
             "crates/bib-base/src/semantic_limits.rs", "crates/bib-base/src/semantic_service/maintenance.rs",
             "crates/text-embedding/src/provider/pinned_ollama.rs", "crates/text-embedding/src/provider/ollama.rs",
             "crates/bib-base/src/local_index.rs", "crates/bib-base/src/local_index/scored_docs.rs",
             "crates/bib-base/src/search_filters.rs", "crates/api-server/examples/semantic_http_benchmark.rs",
             "crates/api-server/src/server.rs", "crates/api-server/src/bib.rs",
             "apps/gui/tests/bibliography-capacity.mjs", "apps/gui/tests/bibliography-capacity.html",
             "apps/gui/src/features/bibliography.tsx", "apps/gui/src/gateway.ts"]
    cpuinfo = Path("/proc/cpuinfo")
    meminfo = Path("/proc/meminfo")
    hardware = {"cpu": next((line.split(":", 1)[1].strip() for line in cpuinfo.read_text().splitlines() if line.startswith("model name")), platform.processor()) if cpuinfo.exists() else platform.processor(),
                "reported_logical_cpus": os.cpu_count(), "allowed_cpus": len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
                "reported_mem_total_kib": next((int(line.split()[1]) for line in meminfo.read_text().splitlines() if line.startswith("MemTotal:")), None) if meminfo.exists() else None,
                "shared_host": True}
    receipt = {"schema": "bib-semantic-service-capacity-runner-v1", "platform": platform.platform(), "hardware": hardware,
               "controlled_http_model_fixture": True, "worker_sha256": hashlib.sha256(worker.read_bytes()).hexdigest(),
               "source_sha256": {p: hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in paths}, "runs": {}}
    try:
        for count in args.counts:
            started = time.monotonic()
            output = args.directory / f"{count}.stdout.log"
            errors = args.directory / f"{count}.stderr.log"
            with lock:
                before = counters.copy()
            extra = []
            reused_receipt = None
            if args.reuse_from:
                source = args.reuse_from / str(count)
                previous = json.loads((source / "receipt.json").read_text())
                if previous.get("count") != count or not previous.get("controlled_http_model_fixture"):
                    raise ValueError("reuse source must be a completed matching synthetic service fixture")
                reused_receipt = hashlib.sha256((source / "receipt.json").read_bytes()).hexdigest()
                shutil.copytree(source, args.directory / str(count), ignore=shutil.ignore_patterns("receipt.json"))
                extra = ["--reuse-existing"]
            cache_advice = None
            if args.evict_file_cache:
                advised_files = 0
                advised_bytes = 0
                for path in (args.directory / str(count)).rglob("*"):
                    if not path.is_file() or path.is_symlink():
                        continue
                    with path.open("rb") as file:
                        os.fsync(file.fileno())
                        os.posix_fadvise(file.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
                        advised_bytes += os.fstat(file.fileno()).st_size
                        advised_files += 1
                cache_advice = {"method": "fsync_then_posix_fadvise_DONTNEED", "files": advised_files,
                                "bytes": advised_bytes, "physical_cache_guaranteed": False}
            with output.open("w") as stdout, errors.open("w") as stderr:
                subprocess.run([str(worker), str(args.directory / str(count)), str(count), origin] + extra,
                               stdout=stdout, stderr=stderr, timeout=900, check=True)
            run = json.loads((args.directory / str(count) / "receipt.json").read_text())
            with lock:
                run["fixture_requests"] = {key: counters[key] - before[key] for key in counters}
            if args.reuse_from:
                if run["fixture_requests"]["embedded_inputs"] > 100:
                    raise AssertionError("copied fixture was re-embedded; cannot claim cache reuse")
                run["reused_source_receipt_sha256"] = reused_receipt
            run["file_cache_advice"] = cache_advice
            run["wall_seconds"] = time.monotonic() - started
            receipt["runs"][str(count)] = run
            (args.directory / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
            print(json.dumps({"count": count, "four_clients": run["four_clients"],
                              "wide": run["single_client"]["wide"], "wall_seconds": run["wall_seconds"]}), flush=True)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


if __name__ == "__main__":
    main()
