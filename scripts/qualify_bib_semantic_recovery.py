#!/usr/bin/env python3
"""Real child SIGKILL + lease recovery against a bounded local model fixture.

Build semantic_process_probe first. Raw documents and fixture databases stay in
an explicitly selected fresh directory. This does not measure model relevance.
"""
import argparse
import hashlib
import http.server
import json
import queue
import subprocess
import threading
import time
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    args.directory.mkdir(exist_ok=False)
    worker = args.worker.resolve()
    lock = threading.Lock()
    release = threading.Event()
    blocked = threading.Event()
    state = {"completed_inputs": 0, "pause": True}

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def reply(self, value):
            payload = json.dumps(value).encode()
            try:
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def do_GET(self):
            assert self.path == "/api/tags"
            self.reply({"models": [{"name": "autonomics-embedding:" + "a" * 64, "digest": "a" * 64}]})

        def do_POST(self):
            assert self.path == "/api/embed"
            length = int(self.headers["Content-Length"])
            assert length <= 256_000
            body = json.loads(self.rfile.read(length))
            assert body["truncate"] is False
            inputs = body["input"]
            assert 1 <= len(inputs) <= 16
            with lock:
                pause = state["pause"] and state["completed_inputs"] >= 36
            if pause:
                blocked.set()
                release.wait(90)
            with lock:
                state["completed_inputs"] += len(inputs)
            self.reply({"embeddings": [[1.0, 2.0] for _ in inputs]})

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    children = []
    stderr_files = []

    def start(mode):
        error_log = (args.directory / f"{mode}.stderr.log").open("w")
        stderr_files.append(error_log)
        child = subprocess.Popen([str(worker), mode, str(args.directory / "library.db"), origin], stdout=subprocess.PIPE, stderr=error_log, text=True, bufsize=1)
        children.append(child)
        events = queue.Queue()

        def read():
            with (args.directory / f"{mode}.jsonl").open("w") as output:
                for line in child.stdout:
                    output.write(line)
                    output.flush()
                    events.put(json.loads(line))

        threading.Thread(target=read, daemon=True).start()
        return child, events

    def wait_for(child, events, predicate):
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            try:
                event = events.get(timeout=0.25)
            except queue.Empty:
                if child.poll() is not None:
                    raise RuntimeError(f"worker exited before expected event: {child.returncode}")
                continue
            if predicate(event):
                return event
        raise TimeoutError("live worker did not reach the expected event")

    try:
        first, events = start("start")
        partial = wait_for(first, events, lambda value: (value.get("maintenance") or {}).get("action") == "repair" and (value.get("progress") or {}).get("processed_chunks") == 16)
        assert blocked.wait(5) and first.poll() is None
        # Let an actual heartbeat persist committed progress before killing.
        time.sleep(1.2)
        assert first.poll() is None
        first.kill()
        first.wait(timeout=10)
        assert first.returncode != 0
        with lock:
            state["pause"] = False
            assert state["completed_inputs"] == 36
        started = time.monotonic()
        recovered, events = start("recover")
        result = wait_for(recovered, events, lambda value: value.get("event") == "recovery_complete")
        assert recovered.wait(timeout=10) == 0
        progress = result["status"]["maintenance"]["progress"]
        assert progress["cached_chunks"] == 16 and progress["embedded_chunks"] == 4
        assert result["status"]["store_generation"] == partial["maintenance"]["store_generation"]
        assert result["query_total"] == 20
        receipt = {"schema": "bib-semantic-maintenance-process-recovery-v1", "signal": "SIGKILL", "controlled_model_http_fixture": True, "relevance_evaluation": False, "partial": partial, "recovered": result, "recovery_seconds_including_remaining_lease": time.monotonic() - started, "worker_sha256": hashlib.sha256(worker.read_bytes()).hexdigest(), "committed_batches_reused": True, "canonical_documents_preserved": 20}
        (args.directory / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"ok": True, "cached_chunks_after_sigkill": 16, "embedded_chunks_after_sigkill": 4, "query_total": 20, "recovery_seconds": receipt["recovery_seconds_including_remaining_lease"]}))
    finally:
        release.set()
        for child in children:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=10)
        server.shutdown()
        server.server_close()
        for handle in stderr_files:
            handle.close()


if __name__ == "__main__":
    main()
