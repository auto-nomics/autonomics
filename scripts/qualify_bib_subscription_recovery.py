#!/usr/bin/env python3
"""Owned real-process API/scheduler recovery; source pages are local fixtures."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid

TOKEN = "subscription-recovery-fixture"
TAIL = "resume-after-fixture-a"
HOSTS = []


def save(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def wait_for(predicate, label, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = predicate()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError("Timed out: " + label)


class Host:
    def __init__(self, worker, root, data, instance, mode):
        self.root, self.instance = root, instance
        self.stdout = (root / f"{instance}.stdout.log").open("w")
        self.stderr = (root / f"{instance}.stderr.log").open("w")
        self.process = subprocess.Popen(
            [str(worker), str(root), data, instance, mode],
            stdin=subprocess.PIPE, stdout=self.stdout, stderr=self.stderr,
            text=True,
        )
        HOSTS.append(self)

        def bootstrap():
            if self.process.poll() is not None:
                raise AssertionError(f"Fixture host {instance} exited {self.process.returncode}: " + (root / f"{instance}.stderr.log").read_text()[-2000:])
            try:
                return json.loads((root / f"{instance}-bootstrap.json").read_text())
            except (FileNotFoundError, json.JSONDecodeError):
                return None

        self.info = wait_for(bootstrap, instance + " bootstrap", 45)
        assert self.info["pid"] == self.process.pid
        assert self.info["origin"].startswith("http://127.0.0.1:")

    def api(self, method, path, body=None, status=None):
        payload = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(
            self.info["origin"] + (path if path.startswith("/__fixture/") else "/api/v1/bib" + path), data=payload, method=method,
            headers={"Authorization": "Bearer " + TOKEN, "Content-Type": "application/json"},
        )
        try:
            response = urllib.request.urlopen(request, timeout=20)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            code, headers = response.code, response.headers
            data = response.read(1024 * 1024 + 1)
        assert len(data) <= 1024 * 1024, "fixture API response exceeded 1MiB"
        result = json.loads(data) if data else None
        if status is not None:
            assert code == status, (method, path, code, result)
        else:
            assert 200 <= code < 300, (method, path, code, result)
        if "/subscriptions" in path:
            assert headers.get("Cache-Control") == "no-store"
        return result

    def rule(self, id):
        return self.api("GET", "/subscriptions/" + id)["rule"]

    def runs(self, id):
        return self.api("GET", "/subscriptions/" + id + "/runs")["runs"]

    def calls(self):
        path = self.root / f"{self.instance}-calls.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def stop(self):
        if self.process.poll() is None:
            self.process.stdin.write("shutdown\n")
            self.process.stdin.flush()
            assert self.process.wait(timeout=20) == 0
        self.close_streams()

    def kill(self):
        assert self.process.poll() is None
        assert self.info["pid"] == self.process.pid
        self.process.kill()  # only this owned Popen handle; never a discovered PID
        assert self.process.wait(timeout=10) == -9
        self.close_streams()

    def close_streams(self):
        if self.process.stdin:
            self.process.stdin.close()
        self.stdout.close()
        self.stderr.close()


def owned(root):
    root.mkdir(parents=True, exist_ok=False)
    save(root / "owned-subscription-fixture.json", {"schema": "bib-subscription-process-fixture-v1", "id": str(uuid.uuid4())})


def spec():
    return {"name": "Process recovery fixture", "enabled": True, "interval_seconds": 3600,
            "request": {"query": "immune", "sources": ["openalex"], "page_size": 1},
            "max_pages_per_run": 10, "max_records_per_run": 10, "run_timeout_ms": 120000}


def terminal(host, id):
    run = host.api("GET", "/subscriptions/runs/" + id)["run"]
    return run if run["state"] != "running" else None


def closed_manifest(directory):
    result = []
    for path in sorted(directory.rglob("*")):
        assert not path.is_symlink(), "owned fixture data unexpectedly contains a symlink"
        if path.is_file():
            result.append({"path": path.relative_to(directory).as_posix(), "bytes": path.stat().st_size,
                           "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    return result


def crash_restore(worker, root):
    owned(root)
    first = Host(worker, root, "data", "first", "hold-tail")
    rule = first.api("POST", "/subscriptions", spec(), 201)["rule"]
    id = rule["id"]
    wait_for(lambda: any(call["cursor"] == TAIL for call in first.calls()), "first host pending second page")
    history = first.runs(id)
    assert len(history) == 1
    initial = first.api("GET", "/subscriptions/runs/" + history[0]["id"])["run"]
    assert initial["state"] == "running" and initial["pages_committed"] == 1 and initial["pages_started"] == 2
    before = first.rule(id)["sources"][0]
    assert before["cursor"] == TAIL and not before["complete"]
    save(root / "before-kill.json", {"run": initial, "rule": first.rule(id), "calls": first.calls()})
    lease = first.api("GET", "/__fixture/lease/" + id)
    assert lease["epoch"] == initial["lease_epoch"] and lease["owner"]
    assert lease["lease_until_ms"] > time.time() * 1000
    save(root / "observed-live-lease.json", lease)
    killed = time.monotonic()
    first.kill()
    print("crash_restore: killed owned host with one committed page and a pending tail", flush=True)
    second = Host(worker, root, "data", "second", "complete")
    time.sleep(1)
    early_observed = time.time() * 1000 + 1500 < lease["lease_until_ms"]
    if early_observed:
        assert not second.calls(), "new process requested a page before the observed old lease expired"
    wait_for(lambda: second.rule(id)["last_success_ms"], "expired lease takeover and completed scan", 45)
    elapsed = time.monotonic() - killed
    history = second.runs(id)
    assert len(history) == 2
    old = second.api("GET", "/subscriptions/runs/" + initial["id"])["run"]
    new = second.api("GET", "/subscriptions/runs/" + history[0]["id"])["run"]
    assert old["state"] == "interrupted" and old["pages_committed"] == 1 and old["reserved_records"] == 0
    assert new["state"] == "complete" and new["records"] == 1 and new["pages_committed"] == 1
    assert new["lease_epoch"] > old["lease_epoch"]
    assert [call["cursor"] for call in second.calls()] == [TAIL]
    assert second.calls()[0]["at_ms"] >= lease["lease_until_ms"], "takeover ran before an observed durable lease deadline"
    assert second.calls()[0]["plan_fingerprint"] == before["plan"]["fingerprint"]
    after = second.rule(id)["sources"][0]
    assert after["scan"]["id"] == before["scan"]["id"] and after["complete"]
    items = second.api("GET", "/subscriptions/inbox")["items"]
    assert len(items) == 2
    a = next(item for item in items if item["records"][0]["article"]["id"] == "fixture-a")
    b = next(item for item in items if item["records"][0]["article"]["id"] == "fixture-b")
    second.api("PUT", "/subscriptions/inbox/" + b["id"], {"read": True}, 204)
    second.api("DELETE", "/subscriptions/inbox/" + a["id"], status=204)
    assert second.api("GET", "/health")["article_count"] == 0
    save(root / "after-takeover.json", {"old_run": old, "new_run": new, "rule": second.rule(id), "calls": second.calls()})
    second.stop()
    source = root / "data"
    manifest = closed_manifest(source)
    shutil.copytree(source, root / "restored-data")  # closed entire family, never just .db
    assert closed_manifest(root / "restored-data") == manifest
    save(root / "closed-backup-manifest.json", manifest)
    print("crash_restore: takeover preserved cursor; closed DB family copied and hashes match", flush=True)
    restored = Host(worker, root, "restored-data", "restored", "complete")
    assert restored.rule(id)["version"] == rule["version"]
    assert len(restored.runs(id)) == 2 and not restored.calls()
    remaining = restored.api("GET", "/subscriptions/inbox")["items"]
    assert len(remaining) == 1 and remaining[0]["id"] == b["id"] and remaining[0]["read"]
    duplicate = restored.api("POST", "/subscriptions/" + id + "/check", {"run_id": new["id"]}, 202)["run"]
    assert duplicate["state"] == "complete" and duplicate["id"] == new["id"] and not restored.calls()
    action = str(uuid.uuid4())
    restored.api("POST", "/subscriptions/" + id + "/check", {"run_id": action}, 202)
    checked = wait_for(lambda: terminal(restored, action), "restored explicit replay", 15)
    assert checked["state"] == "complete" and checked["records"] == 2 and checked["new_notifications"] == 0
    remaining = restored.api("GET", "/subscriptions/inbox")["items"]
    assert len(remaining) == 1 and remaining[0]["id"] == b["id"] and remaining[0]["read"]
    assert [call["cursor"] for call in restored.calls()] == [None, TAIL]
    assert len(restored.runs(id)) == 3
    result = {"passed": True, "deliberate_sigkill": True, "restart_to_complete_seconds": elapsed,
              "prior_run_interrupted": True, "higher_epoch_takeover": True, "resumed_exact_committed_cursor": True,
              "observed_lease_deadline_respected": True, "preexpiry_idle_observation": early_observed,
              "frozen_scan_and_plan_preserved": True, "closed_backup_files": len(manifest),
              "closed_family_hashes_equal": True, "idempotent_action_survived_restore": True,
              "deleted_notification_not_resurrected": True, "read_flag_preserved": True,
              "canonical_article_count": restored.api("GET", "/health")["article_count"]}
    restored.stop()
    return result


def suspended_owner_handoff(worker, root):
    owned(root)
    first = Host(worker, root, "data", "suspended", "hold-tail")
    request = spec()
    request["request"].update(source_timeout_ms=120000, deadline_ms=120000)
    rule = first.api("POST", "/subscriptions", request, 201)["rule"]
    id = rule["id"]
    wait_for(lambda: any(call["cursor"] == TAIL for call in first.calls()), "old owner holding tail")
    initial = first.api("GET", "/subscriptions/runs/" + first.runs(id)[0]["id"])["run"]
    lease = first.api("GET", "/__fixture/lease/" + id)
    assert lease["owner"] and lease["lease_until_ms"] > time.time() * 1000
    assert first.process.poll() is None and first.info["pid"] == first.process.pid
    first.process.send_signal(signal.SIGSTOP)
    print("suspended_owner: stopped owned host to let its durable lease expire", flush=True)
    second = Host(worker, root, "data", "takeover", "complete")
    wait_for(lambda: second.rule(id)["last_success_ms"], "live-process expired lease takeover", 45)
    assert [call["cursor"] for call in second.calls()] == [TAIL]
    assert second.calls()[0]["at_ms"] >= lease["lease_until_ms"]
    assert initial["deadline_ms"] > second.calls()[0]["at_ms"] + 1000, "source/run timeout must not be the reason for cancellation"
    assert len(second.api("GET", "/subscriptions/inbox")["items"]) == 2
    first.process.send_signal(signal.SIGCONT)
    def dropped():
        path = root / "suspended-dropped.jsonl"
        return path.exists() and path.stat().st_size > 0
    wait_for(dropped, "resumed old owner cancels its pending source future", 10)
    old = first.api("GET", "/subscriptions/runs/" + initial["id"])["run"]
    assert old["state"] == "interrupted" and old["records"] == 1 and old["pages_committed"] == 1
    assert len(first.runs(id)) == 2 and len(first.api("GET", "/subscriptions/inbox")["items"]) == 2
    assert [call["cursor"] for call in first.calls()] == [None, TAIL]
    save(root / "handoff.json", {"observed_old_lease": lease, "old_run_after_resume": old,
                                "old_calls": first.calls(), "new_calls": second.calls()})
    first.stop()
    second.stop()
    return {"passed": True, "two_processes_shared_canonical_db": True,
            "takeover_after_observed_lease_deadline": True,
            "old_owner_resumed_before_source_run_timeout": True,
            "old_pending_source_future_cancelled": True,
            "old_run_not_overwritten_after_resume": True,
            "no_duplicate_notifications_or_new_old_owner_requests": True}


def quota_restart(worker, root):
    owned(root)
    first = Host(worker, root, "data", "quota-first", "complete")
    policy = first.api("GET", "/subscriptions/limits")["limits"]
    policy["max_page_bytes"] = 1
    first.api("PUT", "/subscriptions/limits", policy)
    rule = first.api("POST", "/subscriptions", spec(), 201)["rule"]
    id = rule["id"]
    wait_for(lambda: first.rule(id).get("resource_wait"), "persisted quota wait")
    wait_for(lambda: first.runs(id) and first.runs(id)[0]["state"] == "failed", "quota attempt settled")
    assert len(first.calls()) == 1 and len(first.runs(id)) == 1
    first.stop()
    second = Host(worker, root, "data", "quota-second", "complete")
    time.sleep(1.2)
    assert not second.calls() and len(second.runs(id)) == 1 and second.rule(id).get("resource_wait")
    second.api("POST", "/subscriptions/" + id + "/check", {"run_id": str(uuid.uuid4())}, 409)
    assert not second.calls() and len(second.runs(id)) == 1
    policy["max_page_bytes"] = 2 * 1024 * 1024
    second.api("PUT", "/subscriptions/limits", policy)
    wait_for(lambda: second.rule(id)["last_success_ms"], "quota recovery auto-check before regular due time", 15)
    assert len(second.calls()) == 2 and len(second.runs(id)) == 2
    assert len(second.api("GET", "/subscriptions/inbox")["items"]) == 2
    assert not second.rule(id).get("resource_wait")
    result = {"passed": True, "wait_survived_process_restart": True,
              "unchanged_wait_prevented_automatic_and_manual_io": True,
              "history_not_spammed": True, "changed_policy_resumed_before_hourly_due": True,
              "quota_failure_page_was_atomic": True, "canonical_article_count": second.api("GET", "/health")["article_count"]}
    second.stop()
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    assert os.name == "posix", "SIGKILL recovery fixture requires POSIX"
    worker = args.worker.resolve(strict=True)
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    report = {"schema": "bib-subscription-process-recovery-v1", "source_fixture_only": True,
              "real_api_scheduler_and_durable_store": True, "external_requests": False,
              "worker_sha256": hashlib.sha256(worker.read_bytes()).hexdigest()}
    try:
        report["crash_restore"] = crash_restore(worker, directory / "crash-restore")
        report["suspended_owner_handoff"] = suspended_owner_handoff(worker, directory / "suspended-owner")
        report["quota_restart"] = quota_restart(worker, directory / "quota-restart")
        report["passed"] = True
        save(directory / "receipt.json", report)
        print(json.dumps(report), flush=True)
    except Exception as error:
        save(directory / "failure.json", {"error_type": type(error).__name__, "error": str(error)})
        raise
    finally:
        for host in HOSTS:
            if host.process.poll() is None:
                host.process.kill()
                host.process.wait(timeout=10)
            host.close_streams()


if __name__ == "__main__":
    main()
