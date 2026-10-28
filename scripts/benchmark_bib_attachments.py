#!/usr/bin/env python3
"""Bounded synthetic model for true HTTP/VFS attachment fanout qualification.

One worker at a time. No reuse of mutated fixtures, model inference or IR claim.
Timeout is explicit and generous for real extraction/700k-block construction.
"""
import argparse
import hashlib
import http.server
import json
from pathlib import Path
import subprocess
import shutil
import threading
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--counts", type=int, nargs="+", default=[100])
    parser.add_argument("--resume-from", type=Path, help="Copy an incomplete, stopped owned fixture and verify existing attachment bytes before resuming")
    parser.add_argument("--timeout-seconds", type=int, default=14400)
    args = parser.parse_args()
    if any(n < 100 or n % 100 for n in args.counts):
        parser.error("counts must be positive multiples of 100")
    if args.timeout_seconds <= 0:
        parser.error("timeout must be positive")
    worker = args.worker.resolve()
    args.directory.mkdir(exist_ok=False)
    lock = threading.Lock()
    counters = {"embed_requests": 0, "embedded_inputs": 0, "tag_requests": 0}

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
            vectors = [[v - 128 for v in hashlib.sha256(text.encode()).digest()] * 32 for text in inputs]
            with lock:
                counters["embed_requests"] += 1
                counters["embedded_inputs"] += len(inputs)
            self.reply({"embeddings": vectors})

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    paths = ["scripts/benchmark_bib_attachments.py", "crates/api-server/examples/semantic_attachment_benchmark.rs", "Cargo.lock"]
    # Include actual implementation files rather than only the benchmark itself.
    for folder in ["crates/bib-base/src", "crates/api-server/src", "crates/bib-types/src", "crates/text-embedding/src", "crates/vfs/src"]:
        paths.extend(str(p) for p in sorted(Path(folder).rglob("*.rs")))
    receipt = {"schema": "bib-semantic-attachment-runner-v1", "controlled_http_model_fixture": True,
               "worker_sha256": hashlib.sha256(worker.read_bytes()).hexdigest(),
               "source_sha256": {p: hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in paths}, "runs": {}}
    for name in paths:
        snapshot = args.directory / "source_snapshot" / name
        snapshot.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(name, snapshot)
    (args.directory / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    try:
        for count in args.counts:
            start = time.monotonic()
            with lock:
                before = counters.copy()
            extra = []
            if args.resume_from:
                source = args.resume_from / str(count)
                if not (source / "library.db").is_file() or (source / "receipt.json").exists():
                    raise ValueError("resume requires an incomplete fixture, not a completed mutation run")
                shutil.copytree(source, args.directory / str(count))
                extra = ["--resume-existing"]
            receipt["runs"][str(count)] = {"status": "in_progress", "resume_from": str(args.resume_from) if args.resume_from else None}
            (args.directory / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
            with (args.directory / f"{count}.stdout.log").open("w") as stdout, (args.directory / f"{count}.stderr.log").open("w") as stderr:
                try:
                    subprocess.run([str(worker), str(args.directory / str(count)), str(count), origin] + extra,
                                   stdout=stdout, stderr=stderr, timeout=args.timeout_seconds, check=True)
                except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
                    receipt["runs"][str(count)].update(status="failed", error=str(error), wall_seconds=time.monotonic() - start)
                    (args.directory / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
                    raise
            run = json.loads((args.directory / str(count) / "receipt.json").read_text())
            with lock:
                run["fixture_requests"] = {k: counters[k] - before[k] for k in counters}
            run["wall_seconds"] = time.monotonic() - start
            run["status"] = "complete"
            run["resume_from"] = str(args.resume_from) if args.resume_from else None
            receipt["runs"][str(count)] = run
            (args.directory / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
            print(json.dumps({"count": count, "wall_seconds": run["wall_seconds"],
                              "wide": run["single_client"]["wide"], "four_clients": run["four_clients"]}), flush=True)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


if __name__ == "__main__":
    main()
