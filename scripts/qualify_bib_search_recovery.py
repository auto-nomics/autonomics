#!/usr/bin/env python3
"""Kill isolated fixture processes after source commit and during index staging."""
import argparse
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time


def receipt(process, timeout=120):
    messages = queue.Queue()

    def read():
        for line in process.stdout:
            try:
                messages.put(json.loads(line))
                return
            except json.JSONDecodeError:
                continue
        messages.put(RuntimeError(f"probe exited without receipt: {process.poll()}"))

    threading.Thread(target=read, daemon=True).start()
    value = messages.get(timeout=timeout)
    if isinstance(value, Exception):
        raise value
    return value


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--count", type=int, default=1000)
    args = parser.parse_args()
    directory = Path(tempfile.mkdtemp(prefix="bib-search-crash-"))
    database = directory / "library.db"
    receipts = []
    children = []
    try:
        def start(phase):
            log = open(directory / f"{phase}.stderr.log", "w")
            process = subprocess.Popen(
                [args.probe, phase, str(database), str(args.count)],
                stdout=subprocess.PIPE, stderr=log, text=True,
            )
            log.close()
            children.append(process)
            return process

        seed = start("seed")
        receipts.append(receipt(seed))
        if seed.wait(timeout=30) != 0:
            raise RuntimeError("seed failed")
        pending = start("pending")
        receipts.append(receipt(pending))
        if receipts[-1]["phase"] != "committed_pending":
            raise RuntimeError("pending fixture was not committed")
        pending.kill()
        first_exit = pending.wait(timeout=10)
        build = start("build")
        receipts.append(receipt(build))
        if receipts[-1]["phase"] != "building_uncommitted":
            raise RuntimeError("did not interrupt a live uncommitted build")
        build.kill()
        second_exit = build.wait(timeout=10)
        verify = start("verify")
        receipts.append(receipt(verify))
        if verify.wait(timeout=30) != 0 or receipts[-1]["phase"] != "verified":
            raise RuntimeError("recovery verification failed")
        holder = start("hold")
        receipts.append(receipt(holder))
        if receipts[-1]["phase"] != "holding_durable_writer" or not receipts[-1]["status"]["durable"]:
            raise RuntimeError("durable writer holder missing")
        secondary = start("secondary")
        receipts.append(receipt(secondary))
        if secondary.wait(timeout=30) != 0 or receipts[-1]["status"]["durable"]:
            raise RuntimeError("secondary process did not use an independent RAM projection")
        receipts.append(receipt(holder))
        if holder.wait(timeout=30) != 0 or receipts[-1]["phase"] != "observed_secondary_process":
            raise RuntimeError("cross-process replay failed")
        backup = Path(tempfile.mkdtemp(prefix="bib-search-offline-backup-"))
        shutil.copytree(directory, backup, dirs_exist_ok=True)
        shutil.rmtree(backup / "library.db.search-v2")
        restored = subprocess.run([args.probe, "verify", str(backup / "library.db"), str(args.count)], capture_output=True, text=True, timeout=120, check=True)
        restored_receipt = json.loads(restored.stdout.strip())
        if restored_receipt["anchor_example"] != next(r["anchor_example"] for r in receipts if r["phase"] == "verified"):
            raise RuntimeError("backup restore changed evidence identity")
        restored_receipt["case"] = "offline_backup_restore_without_index"
        receipts.append(restored_receipt)
        report = {
            "schema_version": 1, "platform": sys.platform, "process_model": os.name,
            "interruption": "SIGKILL" if os.name == "posix" else "TerminateProcess",
            "interrupted_exit_codes": [first_exit, second_exit],
            "receipts": receipts, "directory": str(directory), "backup_directory": str(backup),
            "recorded_at_unix": time.time(), "result": "passed",
        }
        output = Path(args.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + "\n")
        print(output)
    finally:
        for child in children:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=10)


if __name__ == "__main__":
    main()
