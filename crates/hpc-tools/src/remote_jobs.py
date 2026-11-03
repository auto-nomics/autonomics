"""Owned-account Slurm operations. Python 3.6+, standard library only.

This helper runs via SSH on the login node. It submits scripts to Slurm and
never executes their contents on the login node. Durable intents are separate
from study data and are deliberately retained after completion.
"""
import base64
import datetime
import fcntl
import hashlib
import json
import os
import pwd
import re
import shutil
import socket
import stat
import subprocess
import time
import uuid
from urllib.parse import unquote

MAX_OUTPUT = 2 * 1024 * 1024
TERMINAL = {"COMPLETED", "CANCELLED", "FAILED", "TIMEOUT", "NODE_FAIL",
            "OUT_OF_MEMORY", "BOOT_FAIL", "DEADLINE", "PREEMPTED", "REVOKED"}
JOB_ID = re.compile(r"^[1-9][0-9]{0,11}(?:_[0-9]{1,12})?$")
TOKEN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")
NONCE = re.compile(r"^[a-f0-9]{32}$")


def fail(message):
    raise ValueError(message)


def user():
    return pwd.getpwuid(os.getuid()).pw_name


def cluster_executable(name):
    executable = shutil.which(name)
    # GPFS administration clients are not always on a non-interactive or
    # compute-step PATH. Resolve only the installed system clients we support.
    if executable is None and name in ("mmlsattr", "mmlsquota"):
        candidate = "/usr/lpp/mmfs/bin/" + name
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            executable = candidate
    return executable


def command(argv, timeout=20, required=True):
    # No shell and no user-supplied executable. Output is spooled to an unnamed
    # temporary file to avoid pipe deadlocks and unbounded subprocess memory.
    import tempfile
    env = dict(os.environ)
    env["LC_ALL"] = "C"
    if argv[0] == "sacct":
        env["TZ"] = "UTC"
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        try:
            proc = subprocess.Popen([cluster_executable(argv[0]) or argv[0], *argv[1:]], stdout=out, stderr=err, env=env,
                                    stdin=subprocess.DEVNULL)
        except OSError:
            if required:
                fail("Required cluster command is unavailable: " + argv[0])
            return {"available": False, "returncode": None, "stdout": ""}
        try:
            rc = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
            fail("Cluster command timed out; a mutation may need recovery")
        size = out.tell()
        out.seek(0)
        data = out.read(MAX_OUTPUT + 1)
        if len(data) > MAX_OUTPUT:
            fail("Cluster response exceeded the bounded response limit")
        result = {"available": True, "returncode": rc,
                  "stdout": data.decode("utf-8", "replace"), "output_bytes": size}
        # Never expose arbitrary scheduler stderr (it can include private
        # launcher context). Only fixed messages and strictly parsed identifiers
        # from a small allowlist cross the tool boundary.
        if argv[0] == "sbatch" and rc != 0:
            err.seek(0)
            result["diagnostic"] = submission_diagnostic(err.read(65537))
        if argv[0] in ("quota", "mmlsquota", "mmlsattr", "lfs", "df") and rc != 0:
            err.seek(0)
            diagnostic = err.read(65536).decode("utf-8", "replace").lower()
            code = "quota_provider_failed"
            if "permission denied" in diagnostic or "operation not permitted" in diagnostic:
                code = "permission_denied"
            elif "not found" in diagnostic or "no such" in diagnostic:
                code = "quota_target_not_found"
            result["diagnostic"] = {"code": code, "stderr_bytes": err.seek(0, 2),
                                    "private_stderr_exposed": False}
        return result


def submission_diagnostic(raw):
    result = {"code": "scheduler_rejected_request",
              "message": "Slurm rejected the request; private stderr was not exposed"}
    if len(raw) > 65536:
        return result
    text = raw.decode("utf-8", "replace")
    match = re.search(r"(?m)^sbatch: error: Please submit job from "
                      r"([A-Za-z0-9][A-Za-z0-9.-]{0,252}) when using "
                      r"([A-Za-z0-9][A-Za-z0-9_.-]{0,127}) partition\.$", text)
    if match:
        host, partition = match.groups()
        result = {"code": "login_endpoint_mismatch", "required_login_host": host,
                  "partition": partition,
                  "message": "This partition requires a different login endpoint"}
        if host == "pilogin.hpc.sjtu.edu.cn":
            result["recommended_cluster"] = "pi"
        elif host == "sylogin.hpc.sjtu.edu.cn":
            result["recommended_cluster"] = "siyuan"
    return result


def checked(argv, timeout=20):
    result = command(argv, timeout)
    if result["returncode"] != 0:
        fail("Cluster query failed: " + argv[0])
    return result["stdout"]


def token(value, label):
    if not isinstance(value, str) or not TOKEN.match(value):
        fail("Invalid " + label)
    return value


def job_id(value):
    if not isinstance(value, str) or not JOB_ID.match(value):
        fail("A numeric Slurm job ID or individual array task ID is required")
    return value


def path(value, label):
    if not isinstance(value, str) or not value.startswith("/") or "\x00" in value:
        fail(label + " must be an absolute path")
    if any(part == ".." for part in value.split("/")):
        fail(label + " must not contain parent traversal")
    if len(value) > 4096 or "\n" in value or "\r" in value:
        fail("Invalid " + label)
    return value


def positive(value, label, ceiling=10000000):
    if type(value) is not int or not 1 <= value <= ceiling:
        fail(label + " must be a positive bounded integer")
    return value


def belongs(raw, target):
    # Match a root job, its array tasks and dot steps, never another prefix.
    escaped = re.escape(target)
    if "_" in target:
        return re.match(r"^" + escaped + r"(?:\.[A-Za-z0-9_+-]+)?$", raw) is not None
    return re.match(r"^" + escaped + r"(?:_[0-9]+)?(?:\.[A-Za-z0-9_+-]+)?$", raw) is not None


def parse_accounting(text, target, owner):
    records = []
    for line in text.splitlines():
        if not line.strip():
            continue
        fields = line.split("|")
        if len(fields) != 13:
            fail("Accounting returned a malformed record")
        raw, display, who, state_, exit_code, name, start, end, elapsed, cpus, mem, partition, comment = fields
        if not re.match(r"^[1-9][0-9]{0,11}(?:\.[A-Za-z0-9_+-]+)?$", raw):
            fail("Accounting returned a malformed raw job ID")
        if not re.match(r"^[1-9][0-9]{0,11}(?:_[0-9]{1,12})?(?:\.[A-Za-z0-9_+-]+)?$", display):
            fail("Accounting returned a malformed display job ID")
        if "_" not in display.split(".")[0] and raw != display:
            fail("Regular job raw/display identities do not agree")
        if raw.partition(".")[2] != display.partition(".")[2]:
            fail("Accounting step identities do not agree")
        if not (belongs(display, target) or belongs(raw, target)):
            fail("Accounting returned an unrelated job ID")
        is_step = "." in raw
        if who != owner and not (is_step and who == ""):
            fail("Requested job does not belong to the current account")
        records.append({"job_id": display, "job_id_raw": raw, "user": who, "state": state_,
                        "exit_code": exit_code, "name": name, "start": start,
                        "end": end, "elapsed": elapsed, "cpus": cpus,
                        "memory": mem, "partition": partition, "comment": comment})
    allocations = [r for r in records if "." not in r["job_id_raw"]]
    if records and not allocations:
        fail("Accounting has steps without an owning allocation record")
    raw_allocations = {r["job_id_raw"] for r in allocations}
    for record in records:
        if record["job_id_raw"].split(".")[0] not in raw_allocations:
            fail("Accounting step has no matching raw allocation identity")
    return records


def queue(target=None):
    if target is not None:
        target = job_id(target)
    args = ["squeue", "--noheader", "--array", "--user=" + user(),
            "--format=%i|%u|%T|%j|%k|%R"]
    # Slurm returns nonzero when --jobs names a job already purged from the
    # live controller. Query the complete owned queue and filter locally so a
    # completed job can still be proved from accounting. A failed full query
    # remains a failure, never evidence that the queue is empty.
    rows = []
    for line in checked(args).splitlines():
        if not line.strip():
            continue
        fields = line.strip().split("|")
        if len(fields) != 6:
            fail("Queue returned a malformed record")
        jid, who, state_, name, comment, reason_or_nodes = fields
        job_id(jid)
        if who != user():
            fail("Queue returned a job outside the requested ownership scope")
        if target is not None and not belongs(jid, target):
            continue
        rows.append({"job_id": jid, "user": who, "state": state_,
                     "name": name, "comment": comment,
                     "reason_or_nodes": reason_or_nodes,
                     "pending_reason": reason_or_nodes if state_ == "PENDING" else None})
    return rows


def status(target):
    target = job_id(target)
    live = queue(target)
    result = command(["sacct", "--noheader", "--parsable2", "--duplicates", "--array",
                      "--jobs=" + target, "--user=" + user(),
                      "--format=JobIDRaw,JobID%64,User,State%64,ExitCode,JobName%128,Start,End,Elapsed,AllocCPUS,ReqMem,Partition,Comment%128"])
    if result["returncode"] != 0:
        if live:
            return {"job_id": target, "state": "ACTIVE", "queue": live,
                    "accounting": [], "accounting_available": False,
                    "terminal": False, "stop_confirmed": False, "successful": False}
        fail("Ownership and terminal state cannot be established: accounting unavailable")
    rows = parse_accounting(result["stdout"], target, user())
    if not live and not rows:
        return {"job_id": target, "state": "UNKNOWN", "queue": [],
                "accounting": [], "terminal": False, "stop_confirmed": False,
                "successful": False, "ownership_verified": False}
    terminal = bool(rows) and not live and all(
        r["state"].split()[0].rstrip("+") in TERMINAL and
        re.match(r"^[0-9]+:[0-9]+$", r["exit_code"]) is not None
        for r in rows)
    success = terminal and all(r["state"] == "COMPLETED" and
                               r["exit_code"] == "0:0" for r in rows)
    return {"job_id": target, "state": "TERMINAL" if terminal else "ACTIVE" if live else "UNKNOWN",
            "queue": live, "accounting": rows, "accounting_available": True,
            "ownership_verified": True, "terminal": terminal,
            "stop_confirmed": terminal, "successful": success,
            "terminal_proof_scope": "owned allocation and every returned accounting step; no artifact cleanup authorization"}


def owned(target):
    result = status(target)
    if not result.get("ownership_verified", bool(result["queue"])):
        fail("Job ownership is not verified")
    return result


def history(request):
    limit = request.get("limit", 100)
    positive(limit, "limit", 1000)
    start = request.get("start_time")
    if start is None:
        start = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=7)).strftime("%Y-%m-%dT%H:%M:%S")
    if not isinstance(start, str) or not re.match(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}(?:T[0-9]{2}:[0-9]{2}:[0-9]{2})?$", start):
        fail("start_time must be a UTC ISO date or timestamp")
    text = checked(["sacct", "--noheader", "--parsable2", "--allocations", "--array",
                    "--user=" + user(), "--starttime=" + start,
                    "--format=JobIDRaw,JobID%64,User,State%64,ExitCode,JobName%128,Start,End,Elapsed,AllocCPUS,ReqMem,Partition,Comment%128"])
    result = []
    seen = 0
    for line in text.splitlines():
        if not line.strip():
            continue
        fields = line.split("|")
        if len(fields) != 13:
            fail("History query returned a malformed record")
        target = fields[1]
        job_id(target)
        row = parse_accounting(line, target, user())[0]
        seen += 1
        if len(result) < limit:
            result.append(row)
    return {"allocations": result, "start_time_utc": start, "limit": limit,
            "truncated": seen > limit, "total_returned_by_scheduler": seen,
            "note": "History allocations are descriptive; use status for terminal proof including all accounting steps"}


def secure_dir(directory, create=True):
    # Private durable control state, never a user-provided research directory.
    if create and not os.path.lexists(directory):
        os.mkdir(directory, 0o700)
    info = os.lstat(directory)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        fail("Durable job state directory is not private and owned")


def intent_dir(nonce, create=True):
    if not isinstance(nonce, str) or not NONCE.match(nonce):
        fail("Invalid durable submission nonce")
    home = pwd.getpwuid(os.getuid()).pw_dir
    # Do not chmod existing general-purpose directories or follow symlinks.
    current = home
    for name in (".autonomics-hpc-jobs", nonce):
        current = os.path.join(current, name)
        secure_dir(current, create=create)
    return current


def write_json(directory, name, value):
    temporary = os.path.join(directory, "." + name + ".tmp." + uuid.uuid4().hex)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(temporary, flags, 0o600)
    try:
        with os.fdopen(fd, "w") as output:
            json.dump(value, output, sort_keys=True, separators=(",", ":"))
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, os.path.join(directory, name))
        dfd = os.open(directory, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(dfd)
        finally:
            os.close(dfd)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def read_json(directory, name):
    location = os.path.join(directory, name)
    fd = os.open(location, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | os.O_NONBLOCK)
    with os.fdopen(fd, "r") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            fail("Durable job record is not private and regular")
        # JSON escaping of a bounded 1 MiB script can expand control bytes or
        # Unicode to several MiB. Keep the durable bound above that expansion.
        data = source.read(8 * 1024 * 1024 + 1)
        if len(data) > 8 * 1024 * 1024:
            fail("Durable job record exceeded its bound")
        return json.loads(data)


def normalize_spec(request):
    for field in ("working_directory", "partition", "account", "qos", "constraint",
                  "reservation", "time_limit", "array", "dependency"):
        if request.get(field) in ("", "null"):
            fail(field + ' must not be an empty string or the string "null"; omit it or use JSON null')
    script = request.get("script")
    if not isinstance(script, str) or not script.strip() or len(script.encode("utf-8")) > 1024 * 1024 or "\x00" in script:
        fail("A nonempty batch script of at most 1 MiB is required")
    spec = {"script": script, "working_directory": path(request.get("working_directory"), "working_directory"),
            "partition": token(request.get("partition"), "partition")}
    for field in ("account", "qos", "constraint", "reservation"):
        if request.get(field) is not None:
            spec[field] = token(request[field], field)
    for field in ("nodes", "tasks", "cpus_per_task", "memory_mb", "gpus"):
        if request.get(field) is not None:
            spec[field] = positive(request[field], field)
    if request.get("time_limit") is not None:
        value = request["time_limit"]
        if not isinstance(value, str) or not re.match(r"^(?:[0-9]{1,4}-)?[0-9]{1,5}(?::[0-9]{1,2}){0,2}$", value):
            fail("Invalid Slurm time_limit")
        spec["time_limit"] = value
    if request.get("array") is not None:
        value = request["array"]
        if not isinstance(value, str) or len(value) > 256 or not re.match(r"^[0-9]+(?:-[0-9]+(?::[0-9]+)?)?(?:,[0-9]+(?:-[0-9]+(?::[0-9]+)?)?)*(?:%[1-9][0-9]*)?$", value):
            fail("Invalid Slurm array specification")
        spec["array"] = value
    if request.get("dependency") is not None:
        value = request["dependency"]
        if not isinstance(value, str) or len(value) > 512 or not re.match(r"^(?:after|afterany|afterok|afternotok|aftercorr):[1-9][0-9]*(?::[1-9][0-9]*)*(?:,(?:after|afterany|afterok|afternotok|aftercorr):[1-9][0-9]*(?::[1-9][0-9]*)*)*$", value):
            fail("Invalid Slurm dependency")
        spec["dependency"] = value
    return spec


def adoption(intent):
    expected_name = "autonomics-" + intent["nonce"]
    expected_comment = "autonomics:" + intent["nonce"]
    directory = intent_dir(intent["nonce"], create=False)
    expected_stdout = os.path.join(directory, "stdout-%j.log")
    expected_stderr = os.path.join(directory, "stderr-%j.log")
    matches = set()
    for row in queue():
        if row["name"] == expected_name and row["comment"] == expected_comment:
            matches.add(row["job_id"].split("_")[0])
    result = command(["sacct", "--noheader", "--parsable2", "--allocations", "--array",
                      "--starttime=" + intent["created_at"], "--user=" + user(),
                      "--name=" + expected_name,
                      "--format=JobIDRaw,JobID%64,User,JobName%128,Comment%128,StdOut%4096,StdErr%4096,WorkDir%4096"])
    if result["returncode"] != 0:
        return {"state": "UNKNOWN", "reason": "accounting_unavailable", "submission_retried": False}
    for line in result["stdout"].splitlines():
        if not line.strip():
            continue
        fields = line.split("|")
        if len(fields) != 8:
            fail("Adoption query returned malformed accounting")
        raw, jid, who, name, comment, stdout_path, stderr_path, working_directory = fields
        if who != user():
            fail("Adoption query returned another account's job")
        # AccountingStoreFlags may omit job_comment (as on SJTU). In that
        # case use all three exact durable-path bindings as an independent
        # identity route; a conflicting nonempty comment is never ignored.
        comment_identity = comment == expected_comment
        path_identity = comment in ("", "(null)") and stdout_path == expected_stdout and stderr_path == expected_stderr and working_directory == intent["spec"]["working_directory"]
        if name == expected_name and (comment_identity or path_identity):
            if not re.match(r"^[1-9][0-9]{0,11}$", raw):
                fail("Adoption query returned malformed raw allocation identity")
            job_id(jid)
            if "_" not in jid and raw != jid:
                fail("Adoption raw/display allocation identities do not agree")
            matches.add(jid.split("_")[0])
    if len(matches) != 1:
        return {"state": "UNKNOWN", "reason": "no_exact_match" if not matches else "ambiguous_matches",
                "submission_retried": False, "matches": sorted(matches)}
    return {"state": "SUBMITTED", "job_id": next(iter(matches)),
            "adopted": True, "submission_retried": False,
            "adoption_identity": "exact nonce/name/owner plus comment or all durable stdout/stderr/working-directory paths"}


def sbatch_args(intent, directory):
    spec = intent["spec"]
    args = ["sbatch", "--parsable", "--job-name=autonomics-" + intent["nonce"],
            "--comment=autonomics:" + intent["nonce"],
            "--chdir=" + spec["working_directory"], "--partition=" + spec["partition"],
            "--output=" + os.path.join(directory, "stdout-%j.log"),
            "--error=" + os.path.join(directory, "stderr-%j.log")]
    options = {"account": "account", "qos": "qos", "constraint": "constraint",
               "reservation": "reservation", "nodes": "nodes", "tasks": "ntasks",
               "cpus_per_task": "cpus-per-task", "memory_mb": "mem", "gpus": "gpus",
               "time_limit": "time", "array": "array", "dependency": "dependency"}
    for key, flag in sorted(options.items()):
        if key in spec:
            args.append("--" + flag + "=" + str(spec[key]))
    args.append(os.path.join(directory, "script.sh"))
    return args


def validate_submission(intent, directory):
    """Read-only validation is evidence about NOW, never an old submit receipt."""
    try:
        fd = os.open(os.path.join(directory, "script.sh"),
                     os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as source:
            info = os.fstat(source.fileno())
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                    or info.st_mode & 0o077
                    or source.read(1024 * 1024 + 1) != intent["spec"]["script"].encode("utf-8")):
                fail("Frozen batch script identity mismatch")
        args = sbatch_args(intent, directory)
        args.insert(1, "--test-only")
        result = command(args, timeout=30)
        validation = {"state": "VALIDATED" if result["returncode"] == 0 else "REJECTED",
                      "returncode": result["returncode"], "job_submitted_by_validation": False}
        if result.get("diagnostic"):
            validation["diagnostic"] = result["diagnostic"]
        return validation
    except (ValueError, OSError):
        return {"state": "UNAVAILABLE", "job_submitted_by_validation": False,
                "reason": "validation_unavailable_or_script_identity_mismatch"}


def validate_submission_request(request):
    """Pure syntax validation, before any key/nonce directory is created."""
    try:
        normalize_spec(request)
    except (ValueError, KeyError, TypeError) as error:
        return {"ok": True, "state": "REJECTED", "reason": "invalid_submission_request",
                "diagnostic": str(error)[:512], "submission_attempted": False,
                "submission_retried": False}
    return {"ok": True, "state": "VALIDATED", "submission_attempted": False}


def unresolved_recovery(nonce, reason):
    return {"ok": True, "state": "UNKNOWN", "nonce": nonce, "reason": reason,
            "recovery_action": "operator_review_required", "new_job_submitted": False,
            "submission_retried": False, "automatic_resubmission_permitted": False}


def submit(request, recover=False):
    if not recover:
        validation = validate_submission_request(request)
        if validation["state"] != "VALIDATED":
            return validation
    nonce = request.get("nonce")
    try:
        directory = intent_dir(nonce, create=not recover)
    except FileNotFoundError:
        if recover:
            return unresolved_recovery(nonce, "remote_intent_missing")
        raise
    if recover and not os.path.lexists(os.path.join(directory, "intent.json")):
        # Observation must not manufacture a nonce directory, lock, or intent.
        return unresolved_recovery(nonce, "remote_intent_missing")
    try:
        lock_fd = os.open(os.path.join(directory, "lock"),
                          os.O_RDWR | (0 if recover else os.O_CREAT) | getattr(os, "O_NOFOLLOW", 0), 0o600)
    except FileNotFoundError:
        if recover:
            return unresolved_recovery(nonce, "remote_submission_lock_missing")
        raise
    try:
        lock_info = os.fstat(lock_fd)
        if not stat.S_ISREG(lock_info.st_mode) or lock_info.st_uid != os.getuid() or lock_info.st_mode & 0o077:
            fail("Durable submission lock is unsafe")
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except (IOError, OSError):
            return {"ok": True, "state": "BUSY", "nonce": nonce, "submission_retried": False}
        record_path = os.path.join(directory, "intent.json")
        if os.path.exists(record_path):
            intent = read_json(directory, "intent.json")
            if intent.get("version") != 1 or intent.get("nonce") != nonce or intent.get("owner_uid") != os.getuid():
                fail("Durable submission identity mismatch")
            frozen_spec = normalize_spec(intent["spec"])
            frozen_digest = hashlib.sha256(json.dumps(frozen_spec, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest()
            if frozen_spec != intent["spec"] or frozen_digest != intent["request_sha256"]:
                fail("Durable submission request identity mismatch")
            if recover and request.get("expected_request") is not None:
                if normalize_spec(request["expected_request"]) != frozen_spec:
                    fail("Recovered intent differs from the expected local submission request")
            if recover:
                for field in ("script", "working_directory", "partition", "account", "qos", "constraint",
                              "reservation", "nodes", "tasks", "cpus_per_task", "memory_mb", "gpus",
                              "time_limit", "array", "dependency"):
                    if request.get(field) is not None and request[field] != frozen_spec.get(field):
                        fail("recover cannot change frozen " + field + "; use the original value or recover with only the original key and cluster")
            if not isinstance(intent.get("created_at"), str) or not re.match(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}$", intent["created_at"]):
                fail("Durable submission timestamp is invalid")
            if not recover:
                spec = normalize_spec(request)
                digest = hashlib.sha256(json.dumps(spec, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest()
                if digest != intent["request_sha256"]:
                    fail("Submission key was already bound to a different script or resources")
            if intent["state"] == "SUBMITTED":
                return {"ok": True, "state": "SUBMITTED", "job_id": intent["job_id"],
                        "nonce": nonce, "adopted": False, "submission_retried": False}
            if intent["state"] in ("SUBMITTING", "UNKNOWN"):
                result = adoption(intent)
                if result.get("reason") == "no_exact_match":
                    # Diagnose old ambiguous records without weakening their
                    # no-retry rule or treating today's rejection as proof that
                    # the original attempt never reached the scheduler.
                    result["current_validation"] = validate_submission(intent, directory)
                if intent.get("submission_diagnostic"):
                    result["submission_diagnostic"] = intent["submission_diagnostic"]
                if "submission_returncode" in intent:
                    result["submission_returncode"] = intent["submission_returncode"]
                intent.update(result)
                write_json(directory, "intent.json", intent)
                result.update({"ok": True, "nonce": nonce})
                return result
            if intent["state"] != "PREPARED":
                fail("Durable submission state is invalid")
            spec = intent["spec"]
        else:
            if recover:
                return unresolved_recovery(nonce, "remote_intent_missing")
            spec = normalize_spec(request)
            digest = hashlib.sha256(json.dumps(spec, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest()
            intent = {"version": 1, "nonce": nonce, "owner_uid": os.getuid(),
                      "created_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S"),
                      "request_sha256": digest, "spec": spec, "state": "PREPARED"}
            write_json(directory, "intent.json", intent)
        if recover:
            result = {"ok": True, "state": "PREPARED", "nonce": nonce,
                      "reason": "submission_not_attempted", "submission_retried": False}
            if intent.get("current_validation"):
                result["current_validation"] = intent["current_validation"]
            return result
        if not os.path.isdir(spec["working_directory"]):
            fail("Batch working directory does not exist or is inaccessible")
        script_path = os.path.join(directory, "script.sh")
        script_tmp = os.path.join(directory, ".script." + uuid.uuid4().hex)
        script_fd = os.open(script_tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        with os.fdopen(script_fd, "wb") as output:
            output.write(spec["script"].encode("utf-8"))
            output.flush()
            os.fsync(output.fileno())
        # PREPARED proves sbatch has not been attempted, so an interrupted
        # script write can be safely replaced before crossing SUBMITTING.
        os.replace(script_tmp, script_path)
        validation = validate_submission(intent, directory)
        intent["current_validation"] = validation
        write_json(directory, "intent.json", intent)
        if validation["state"] != "VALIDATED":
            # The durable state remains PREPARED: no real sbatch was attempted.
            return {"ok": True, "state": validation["state"], "nonce": nonce,
                    "reason": "submission_validation_failed", "current_validation": validation,
                    "submission_attempted": False, "submission_retried": False}
        args = sbatch_args(intent, directory)
        intent["state"] = "SUBMITTING"
        write_json(directory, "intent.json", intent)
        # From this point no failure permits automatic resubmission.
        result = command(args, timeout=30)
        intent["submission_returncode"] = result["returncode"]
        raw_id = result["stdout"].strip().split(";")[0]
        if result["returncode"] != 0 or not JOB_ID.match(raw_id) or "_" in raw_id:
            intent["state"] = "UNKNOWN"
            intent["reason"] = "submission_reply_unconfirmed"
            if result.get("diagnostic"):
                intent["submission_diagnostic"] = result["diagnostic"]
            write_json(directory, "intent.json", intent)
            response = {"ok": True, "state": "UNKNOWN", "nonce": nonce,
                        "reason": intent["reason"], "submission_retried": False,
                        "submission_returncode": result["returncode"]}
            if intent.get("submission_diagnostic"):
                response["submission_diagnostic"] = intent["submission_diagnostic"]
            return response
        intent.update({"state": "SUBMITTED", "job_id": raw_id})
        write_json(directory, "intent.json", intent)
        return {"ok": True, "state": "SUBMITTED", "job_id": raw_id,
                "nonce": nonce, "adopted": False, "submission_retried": False,
                "stdout_pattern": os.path.join(directory, "stdout-%j.log"),
                "stderr_pattern": os.path.join(directory, "stderr-%j.log")}
    finally:
        os.close(lock_fd)


def log_read(request):
    target = job_id(request.get("job_id"))
    owned(target)
    stream = request.get("stream", "stdout")
    if stream not in ("stdout", "stderr"):
        fail("stream must be stdout or stderr")
    explicit = request.get("path")
    if explicit is None:
        # Slurm's one-line format preserves spaces in values. Bound the value by
        # its next documented field, then require one unambiguous match.
        data = checked(["scontrol", "show", "job", "--oneliner", target])
        key = "StdOut" if stream == "stdout" else "StdErr"
        values = re.findall(r"(?:^|\s)" + key + r"=(.*?)(?=\s[A-Za-z][A-Za-z0-9_]*=|$)", data)
        if len(values) != 1:
            fail("Log path unavailable; supply its absolute path explicitly")
        explicit = values[0]
    filename = path(explicit, "log path")
    limit = request.get("max_bytes", 65536)
    positive(limit, "max_bytes", 1024 * 1024)
    offset = request.get("offset", 0)
    if type(offset) is not int or offset < 0 or offset > 2 ** 63 - 1:
        fail("Invalid log offset")
    fd = os.open(filename, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(fd, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode):
            fail("Log must be a regular file")
        source.seek(offset)
        data = source.read(limit)
    return {"job_id": target, "path": filename, "stream": stream, "offset": offset,
            "next_offset": offset + len(data), "bytes": len(data),
            "truncated": offset + len(data) < info.st_size,
            "content": data.decode("utf-8", "replace"), "base64": base64.b64encode(data).decode("ascii"),
            "sha256": hashlib.sha256(data).hexdigest()}


def verify_operator_followup(request):
    """Internal read-only verification of a host-operator-bound diagnostic."""
    nonce = request.get("nonce")
    directory = intent_dir(nonce)
    intent = read_json(directory, "intent.json")
    expected = normalize_spec(request.get("expected_request", {}))
    frozen = normalize_spec(intent["spec"])
    frozen_digest = hashlib.sha256(json.dumps(frozen, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest()
    target = job_id(request.get("expected_job_id"))
    if (intent.get("version") != 1 or intent.get("nonce") != nonce
            or intent.get("owner_uid") != os.getuid()
            or intent.get("state") != "SUBMITTED" or intent.get("job_id") != target
            or expected != frozen or frozen_digest != intent.get("request_sha256")):
        fail("Operator followup remote intent identity mismatch")
    fd = os.open(os.path.join(directory, "script.sh"), os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            fail("Operator followup batch script is unsafe")
        script = stream.read(1024 * 1024 + 1)
        if script != frozen["script"].encode("utf-8"):
            fail("Operator followup batch script identity mismatch")
    actual = status(target)
    allocations = [row for row in actual.get("accounting", []) if "." not in row["job_id_raw"]]
    if ((not allocations and not actual.get("queue"))
            or any(row["name"] != "autonomics-" + nonce for row in allocations)
            or any(row["name"] != "autonomics-" + nonce or row["comment"] != "autonomics:" + nonce for row in actual.get("queue", []))):
        fail("Operator followup accounting nonce does not match")
    return {"nonce": nonce, "job_id": target, "remote_intent_verified": True,
            "script_sha256": hashlib.sha256(script).hexdigest(), "status": actual}


def quota_query(argv):
    """A failed optional provider must not erase the filesystem observation."""
    try:
        return command(argv, required=False)
    except ValueError:
        return {"available": True, "returncode": None, "stdout": "",
                "diagnostic": {"code": "quota_query_incomplete"}}


def gpfs_record(result, scope, identity, fileset):
    """Decode the header-labelled GPFS -Y protocol; never infer limits from rc=0."""
    if result.get("returncode") != 0:
        fail("GPFS " + scope + " query failed")
    header = None
    records = []
    prefix = ["mmlsquota", scope]
    for line in result.get("stdout", "").splitlines():
        fields = line.split(":")
        if fields[:2] != prefix:
            fail("Unexpected GPFS quota record")
        if fields[2:3] == ["HEADER"]:
            if header is not None:
                fail("Duplicate GPFS quota header")
            header = fields
            continue
        if header is None or len(fields) != len(header):
            fail("Malformed GPFS quota record")
        # Only named payload columns; repeated reserved fields are not data.
        record = {key: unquote(value) for key, value in zip(header[6:-1], fields[6:-1])}
        records.append(record)
    if scope == "fileset":
        matches = [r for r in records if r.get("name") == fileset and r.get("quotaType") == "FILESET"]
    else:
        kind = {"user": "USR", "group": "GRP"}[scope]
        matches = [r for r in records if r.get("id") == str(identity)
                   and r.get("quotaType") == kind and r.get("filesetname") == fileset]
    if len(matches) != 1 or len(records) != 1:
        fail("GPFS quota scope is absent or ambiguous")
    record = matches[0]
    for key in ("blockUsage", "blockQuota", "blockLimit", "blockInDoubt",
                "filesUsage", "filesQuota", "filesLimit", "filesInDoubt"):
        if not re.fullmatch(r"[0-9]+", record.get(key, "")):
            fail("Non-numeric GPFS quota quantity")
        record[key] = int(record[key])
    # GPFS -Y block quantities are KiB, not bytes. Zero means no limit
    # in THIS scope only; user unlimited never overrides a fileset cap.
    def dimension(usage, soft, hard, doubt, scale):
        limits = [n for n in (soft, hard) if n > 0]
        return {"usage": usage * scale, "soft_limit": soft * scale,
                "hard_limit": hard * scale, "in_doubt": doubt * scale,
                "remaining_conservative": max(0, (min(limits) - usage - doubt) * scale) if limits else None,
                "limit_state": "LIMITED" if limits else "NO_LIMIT_IN_THIS_SCOPE"}
    return {"scope": scope, "identity": str(identity), "fileset": fileset,
            "filesystem": record["filesystemName"],
            "bytes": dimension(*(record[k] for k in ("blockUsage", "blockQuota", "blockLimit", "blockInDoubt")), scale=1024),
            "files": dimension(*(record[k] for k in ("filesUsage", "filesQuota", "filesLimit", "filesInDoubt")), scale=1)}


def quota_snapshot(request):
    location = path(request.get("path", pwd.getpwuid(os.getuid()).pw_dir), "quota path")
    queries = {"filesystem": quota_query(["df", "-P", "-T", "-k", "--", location])}
    result = {"path": location, "observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "queries": queries, "quota_verified": False, "scopes": [],
              "qualification": "RAW_PROVIDER_OUTPUT_ONLY", "reservation": False,
              "note": "Quota is an observation, not reserved capacity or research authorization"}
    try:
        fs = queries["filesystem"]
        lines = fs["stdout"].splitlines()
        if fs["returncode"] != 0 or len(lines) != 2:
            fail("Filesystem identity unavailable")
        fields = lines[1].split()
        if len(fields) != 7 or not fields[4].isdigit():
            fail("Filesystem identity is ambiguous")
        device, fs_type = fields[:2]
        result["filesystem_available_bytes"] = int(fields[4]) * 1024
        result["filesystem_type"] = fs_type
        if fs_type != "gpfs":
            queries["user_quota"] = quota_query(["quota", "-s"])
            if fs_type == "lustre" and shutil.which("lfs"):
                queries["lustre_quota"] = quota_query(["lfs", "quota", "-u", user(), location])
            fail("This filesystem has no qualified quota parser; raw provider output retained")
        token(device, "GPFS device")
        queries["gpfs_attributes"] = quota_query(["mmlsattr", "-L", location])
        attrs = queries["gpfs_attributes"]
        found = re.findall(r"(?m)^fileset name:\s*(\S+)\s*$", attrs["stdout"])
        if attrs["returncode"] != 0 or len(found) != 1:
            fail("GPFS fileset identity unavailable")
        fileset = token(found[0], "GPFS fileset")
        scoped_device = device + ":" + fileset
        queries["gpfs_quota"] = quota_query(["mmlsquota", "-Y", "-v", "-u", str(os.getuid()), scoped_device])
        scopes = [gpfs_record(queries["gpfs_quota"], "user", os.getuid(), fileset)]
        # Include the destination GID (setgid directories) as well as caller
        # groups. Every applicable group scope must be observed successfully.
        gids = sorted(set(os.getgroups()) | {os.getgid(), os.stat(location).st_gid})
        for gid in gids:
            key = "gpfs_group_" + str(gid)
            queries[key] = quota_query(["mmlsquota", "-Y", "-v", "-g", str(gid), scoped_device])
            scopes.append(gpfs_record(queries[key], "group", gid, fileset))
        queries["gpfs_fileset"] = quota_query(["mmlsquota", "-Y", "-j", fileset, device])
        scopes.append(gpfs_record(queries["gpfs_fileset"], "fileset", fileset, fileset))
        if len({item["filesystem"] for item in scopes}) != 1:
            fail("Quota providers disagree on filesystem identity")
        result["scopes"] = scopes
        result["quota_verified"] = True
        result["qualification"] = "GPFS_USER_GROUP_FILESET_SNAPSHOT"
        for dim in ("bytes", "files"):
            candidates = [s[dim]["remaining_conservative"] for s in scopes
                          if s[dim]["remaining_conservative"] is not None]
            result["quota_remaining_" + dim] = min(candidates) if candidates else None
        result["available_bytes_conservative"] = min(
            result["filesystem_available_bytes"], result["quota_remaining_bytes"]
        ) if result["quota_remaining_bytes"] is not None else result["filesystem_available_bytes"]
    except (ValueError, OSError) as error:
        result["reason"] = str(error) if isinstance(error, ValueError) else "Quota path metadata unavailable"
    result["successful_quota_queries"] = [k for k, v in queries.items()
        if k not in ("filesystem", "gpfs_attributes") and v.get("returncode") == 0]
    return result


def cluster(request):
    action = request.get("action", "identity")
    if action == "identity":
        account = pwd.getpwuid(os.getuid())
        return {"user": account.pw_name, "uid": os.getuid(), "gid": os.getgid(),
                "groups": os.getgroups(), "home": account.pw_dir,
                "host": socket.gethostname(), "tools": {name: cluster_executable(name) is not None
                 for name in ("sbatch", "squeue", "sacct", "scontrol", "sinfo", "sacctmgr", "lfs", "mmlsquota", "quota")}}
    if action == "queue":
        return {"jobs": queue(), "user": user()}
    if action == "partitions":
        return command(["sinfo", "--noheader", "--format=%P|%a|%l|%D|%c|%m|%G|%f"])
    if action == "accounts":
        return command(["sacctmgr", "--noheader", "--parsable2", "show", "associations",
                        "where", "user=" + user(), "format=Cluster,Account,User,Partition,QOS,DefaultQOS"], required=False)
    if action == "resources":
        args = ["sinfo", "--Node", "--noheader", "--format=%N|%P|%t|%c|%C|%m|%e|%G|%f"]
        if request.get("partition") is not None:
            args.append("--partition=" + token(request["partition"], "partition"))
        return command(args)
    if action == "quota":
        return quota_snapshot(request)
    fail("Unknown cluster action")


def jobs(request):
    action = request.get("action")
    if action == "validate_submit":
        # Before local key binding: no intent directory, script, scheduler call,
        # or job creation. Use the actual submission parser to avoid drift.
        normalize_spec(request)
        return {"request_validated": True, "submission_attempted": False}
    if action == "validate_submission_request":
        return validate_submission_request(request)
    if action in ("verify_operator_followup", "verify_job_intent"):
        return verify_operator_followup(request)
    if action == "submit":
        return submit(request)
    if action == "recover":
        return submit(request, recover=True)
    if action == "list":
        result = {"jobs": queue(), "user": user()}
        if request.get("include_completed", False) is True:
            result["history"] = history(request)
        return result
    if action == "status":
        return status(request.get("job_id"))
    if action == "logs":
        return log_read(request)
    if action in ("cancel", "hold", "release", "requeue", "update"):
        target = job_id(request.get("job_id"))
        before = owned(target)
        if action == "cancel":
            if before["stop_confirmed"]:
                return {"job_id": target, "cancel_requested": False,
                        "stop_confirmed": True, "status": before}
            checked(["scancel", target])
            after = before
            for attempt in range(4):
                if attempt:
                    time.sleep(1)
                after = status(target)
                if after["stop_confirmed"]:
                    break
            return {"job_id": target, "cancel_requested": True,
                    "stop_confirmed": after["stop_confirmed"], "status": after,
                    "state": "STOP_CONFIRMED" if after["stop_confirmed"] else "STOP_PENDING"}
        if action == "update":
            value = request.get("time_limit")
            normalized = normalize_spec({"script": "#!/bin/sh\ntrue\n", "working_directory": "/", "partition": "unused", "time_limit": value})
            if "time_limit" not in normalized:
                fail("update requires time_limit")
            checked(["scontrol", "update", "JobId=" + target, "TimeLimit=" + normalized["time_limit"]])
        else:
            checked(["scontrol", action, target])
        return {"job_id": target, "action": action, "request_accepted": True,
                "status": status(target), "operation_scope": "current Unix account only"}
    fail("Unknown jobs action")


def run(request):
    try:
        if not isinstance(request, dict):
            fail("Request must be a JSON object")
        result = cluster(request) if request.get("tool") == "cluster" else jobs(request)
        result.setdefault("ok", True)
        # Scientific admission cannot be conferred by an account tool.
        result["scientific_admission_granted"] = False
        return result
    except (ValueError, OSError, KeyError, TypeError) as error:
        return {"ok": False, "error": str(error)[:512], "scientific_admission_granted": False}


if "REQUEST" in globals():
    print(json.dumps(run(REQUEST), sort_keys=True, separators=(",", ":")))
