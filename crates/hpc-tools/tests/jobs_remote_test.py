"""Self-cleaning toy scheduler tests; never contact SSH or a real scheduler."""
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch


HELPER = Path(__file__).resolve().parents[1] / "src" / "remote_jobs.py"


def load_helper():
    spec = importlib.util.spec_from_file_location("remote_jobs", str(HELPER))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Scheduler:
    def __init__(self, helper):
        self.helper = helper
        self.calls = []
        self.live = []
        self.rows = []
        self.submissions = 0
        self.lost_reply = False
        self.bad_reply = False
        self.accounting_available = True
        self.stuck_cancel = False
        self.logs = ""
        self.raw_ids = {}
        self.output_paths = {}
        self.fail_queue_with_job_filter = False
        self.fail_global_queue = False
        self.validation_rejected = False
        self.submit_rejected = False

    def result(self, stdout="", rc=0):
        return {"available": True, "returncode": rc, "stdout": stdout}

    def __call__(self, argv, timeout=20, required=True):
        self.calls.append(list(argv))
        if argv[0] == "sbatch":
            if "--test-only" in argv:
                result = self.result(rc=1 if self.validation_rejected else 0)
                if self.validation_rejected:
                    result["diagnostic"] = {"code": "login_endpoint_mismatch",
                        "required_login_host": "pilogin.hpc.sjtu.edu.cn", "recommended_cluster": "pi"}
                return result
            self.submissions += 1
            if self.submit_rejected:
                return {**self.result(rc=1), "diagnostic": {"code": "scheduler_rejected_request"}}
            name = next(v.split("=", 1)[1] for v in argv if v.startswith("--job-name="))
            comment = next(v.split("=", 1)[1] for v in argv if v.startswith("--comment="))
            self.live = [["123", "toyuser", "RUNNING", name, comment]]
            self.rows = [["123", "toyuser", "RUNNING", "0:0", name,
                          "2026-10-24T01:00:00", "Unknown", "00:00:01", "1", "64M", "cpu", comment]]
            self.output_paths["123"] = [next(v.split("=", 1)[1] for v in argv if v.startswith("--output=")),
                                        next(v.split("=", 1)[1] for v in argv if v.startswith("--error=")),
                                        next(v.split("=", 1)[1] for v in argv if v.startswith("--chdir="))]
            if self.lost_reply:
                raise ValueError("Transport reply lost after scheduler accepted job")
            return self.result("unparseable" if self.bad_reply else "123;testcluster\n")
        if argv[0] == "squeue":
            if self.fail_global_queue or (self.fail_queue_with_job_filter and any(arg.startswith("--jobs=") for arg in argv)):
                return self.result(rc=1)
            return self.result("\n".join("|".join(row if len(row) == 6 else row + ["toy-node"]) for row in self.live))
        if argv[0] == "sacct":
            if not self.accounting_available:
                return self.result(rc=1)
            adoption = any("JobID%64,User,JobName" in item for item in argv)
            rows = [[self.raw_ids.get(r[0], r[0].split("_")[0]), r[0], r[1], r[4], r[11]] + self.output_paths.get(r[0].split("_")[0], ["", "", ""]) for r in self.rows] if adoption else [
                [self.raw_ids.get(r[0], r[0].replace("_4", "")), r[0]] + r[1:] for r in self.rows]
            return self.result("\n".join("|".join(row) for row in rows))
        if argv[0] == "scancel":
            if not self.stuck_cancel:
                self.live = []
                for row in self.rows:
                    row[2] = "CANCELLED"
                    row[3] = "0:15"
            return self.result()
        if argv[0] == "scontrol":
            return self.result(self.logs)
        return self.result()


class JobsTests(unittest.TestCase):
    def test_parameter_preflight_has_no_intent_or_scheduler_side_effects(self):
        for field in ("array", "dependency", "qos", "account", "constraint", "reservation"):
            for value in ("null", ""):
                with self.subTest(field=field, value=value):
                    result = self.invoke(action="validate_submit", **{field: value})
                    self.assertFalse(result["ok"])
                    self.assertIn(field, result["error"])
                    self.assertIn("JSON null", result["error"])
        self.assertFalse(self.invoke(action="validate_submit", array="0;evil")["ok"])
        valid = self.invoke(action="validate_submit", array=None)
        self.assertTrue(valid["request_validated"])
        self.assertFalse(valid["submission_attempted"])
        self.assertEqual(self.scheduler.calls, [])
        self.assertEqual(list(Path(self.temp.name).iterdir()), [])

    def test_recovery_rejects_changed_script_and_resources_without_mutation(self):
        self.assertEqual(self.invoke()["state"], "SUBMITTED")
        intent = Path(self.helper.intent_dir(self.nonce), "intent.json")
        before = intent.read_bytes()
        calls = list(self.scheduler.calls)
        for field, value in (("script", "#!/bin/sh\nfalse\n"), ("memory_mb", 8192),
                             ("dependency", "afterok:456")):
            result = self.invoke(action="recover", **{field: value})
            self.assertFalse(result["ok"])
            self.assertIn("recover cannot change frozen " + field, result["error"])
        self.assertEqual(intent.read_bytes(), before)
        self.assertEqual(self.scheduler.calls, calls)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.helper = load_helper()
        self.scheduler = Scheduler(self.helper)
        self.helper.command = self.scheduler
        self.helper.user = lambda: "toyuser"
        account = types.SimpleNamespace(pw_name="toyuser", pw_dir=self.temp.name)
        self.helper.pwd = types.SimpleNamespace(getpwuid=lambda _: account)
        self.nonce = "e" * 32
        self.request = {"tool": "jobs", "action": "submit", "nonce": self.nonce,
                        "script": "#!/bin/sh\nprintf 'toy'\n", "partition": "cpu",
                        "working_directory": self.temp.name, "cpus_per_task": 1,
                        "memory_mb": 64, "time_limit": "00:01:00"}

    def invoke(self, **kwargs):
        request = dict(self.request)
        request.update(kwargs)
        return self.helper.run(request)

    def terminal(self, state="COMPLETED", exit_code="0:0"):
        self.invoke()
        self.scheduler.live = []
        self.scheduler.rows[0][2:4] = [state, exit_code]

    def followup(self, **changes):
        request = {"tool": "jobs", "action": "verify_operator_followup", "nonce": self.nonce,
                   "expected_request": self.request, "expected_job_id": "123"}
        request.update(changes)
        return self.helper.run(request)

    def test_operator_followup_checks_frozen_intent_script_and_success_without_submission(self):
        self.terminal()
        calls = len(self.scheduler.calls)
        result = self.followup()
        self.assertTrue(result["remote_intent_verified"])
        self.assertTrue(result["status"]["successful"])
        self.assertTrue(result["status"]["stop_confirmed"])
        self.assertEqual(self.scheduler.submissions, 1)
        self.assertFalse(any(c[0] == "sbatch" for c in self.scheduler.calls[calls:]))

    def test_operator_followup_rejects_rebound_script_or_resources(self):
        self.terminal()
        for field, value in [("script", "#!/bin/sh\nfalse\n"), ("memory_mb", 128)]:
            self.assertFalse(self.followup(expected_request={**self.request, field: value})["ok"])

    def test_operator_followup_rejects_wrong_job_and_accounting_nonce(self):
        self.terminal()
        self.assertFalse(self.followup(expected_job_id="456")["ok"])
        self.scheduler.rows[0][4] = "autonomics-" + "f" * 32
        self.assertFalse(self.followup()["ok"])

    def test_operator_followup_rejects_modified_remote_script(self):
        self.terminal()
        Path(self.helper.intent_dir(self.nonce), "script.sh").write_text("#!/bin/sh\nfalse\n")
        self.assertFalse(self.followup()["ok"])

    def test_operator_followup_retains_unsuccessful_or_active_status(self):
        self.invoke()
        self.assertFalse(self.followup()["status"]["stop_confirmed"])
        self.scheduler.live = []
        self.scheduler.rows[0][2:4] = ["FAILED", "1:0"]
        self.assertFalse(self.followup()["status"]["successful"])

    def test_monitor_can_verify_live_job_before_accounting_appears(self):
        self.invoke()
        self.scheduler.rows = []
        result = self.followup(action="verify_job_intent")
        self.assertTrue(result["ok"])
        self.assertFalse(result["status"]["terminal"])
        self.assertFalse(result["status"]["stop_confirmed"])
        self.assertEqual(self.scheduler.submissions, 1)

    def test_monitor_rejects_live_nonce_rebinding_without_accounting(self):
        self.invoke()
        self.scheduler.rows = []
        self.scheduler.live[0][4] = "autonomics:" + "f" * 32
        self.assertFalse(self.followup(action="verify_job_intent")["ok"])

    def test_monitor_requires_terminal_accounting_when_queue_disappears(self):
        self.invoke()
        self.scheduler.rows = []
        self.scheduler.live = []
        self.assertFalse(self.followup(action="verify_job_intent")["ok"])

    def test_submit_is_persisted_and_repeat_does_not_submit_again(self):
        first = self.invoke()
        self.assertEqual(first["job_id"], "123")
        second = self.invoke()
        self.assertEqual(second["job_id"], "123")
        self.assertEqual(self.scheduler.submissions, 1)
        directory = Path(self.helper.intent_dir(self.nonce))
        self.assertEqual((directory / "intent.json").stat().st_mode & 0o777, 0o600)
        self.assertEqual((directory / "script.sh").stat().st_mode & 0o777, 0o600)
        self.assertFalse(first["scientific_admission_granted"])

    def test_wrong_endpoint_is_rejected_before_submission_and_key_stays_prepared(self):
        self.scheduler.validation_rejected = True
        result = self.invoke()
        self.assertEqual(result["state"], "REJECTED")
        self.assertFalse(result["submission_attempted"])
        self.assertEqual(result["current_validation"]["diagnostic"]["recommended_cluster"], "pi")
        self.assertEqual(self.scheduler.submissions, 0)
        intent = self.helper.read_json(self.helper.intent_dir(self.nonce), "intent.json")
        self.assertEqual(intent["state"], "PREPARED")
        self.assertEqual(self.invoke(action="recover")["state"], "PREPARED")
        self.scheduler.validation_rejected = False
        self.assertEqual(self.invoke()["state"], "SUBMITTED")
        self.assertEqual(self.scheduler.submissions, 1)

    def test_validation_uses_all_real_submission_arguments_without_running_script(self):
        self.invoke(account="toyaccount", qos="normal", array="0-2", dependency="afterok:100")
        validation, actual = [c for c in self.scheduler.calls if c[0] == "sbatch"]
        self.assertEqual(validation, [actual[0], "--test-only"] + actual[1:])

    def test_validation_failure_cannot_cross_submitting_boundary(self):
        original = self.scheduler
        def unavailable(argv, **kwargs):
            if argv[0] == "sbatch":
                raise ValueError("Private validation error")
            return original(argv, **kwargs)
        self.helper.command = unavailable
        self.assertEqual(self.invoke()["state"], "UNAVAILABLE")
        self.assertEqual(self.scheduler.submissions, 0)
        intent = self.helper.read_json(self.helper.intent_dir(self.nonce), "intent.json")
        self.assertEqual(intent["state"], "PREPARED")

    def test_legacy_unknown_gets_diagnosis_without_rewriting_past_or_resubmitting(self):
        self.scheduler.bad_reply = True
        self.invoke()
        self.scheduler.live = []
        self.scheduler.rows = []
        self.scheduler.validation_rejected = True
        result = self.invoke(action="recover")
        self.assertEqual(result["state"], "UNKNOWN")
        self.assertEqual(result["reason"], "no_exact_match")
        self.assertEqual(result["current_validation"]["state"], "REJECTED")
        self.assertEqual(self.scheduler.submissions, 1)
        self.assertEqual(self.invoke()["state"], "UNKNOWN")
        self.assertEqual(self.scheduler.submissions, 1)

    def test_real_submit_error_after_validation_remains_unknown_with_durable_diagnostic(self):
        self.scheduler.submit_rejected = True
        result = self.invoke()
        self.assertEqual(result["state"], "UNKNOWN")
        self.assertEqual(result["submission_diagnostic"]["code"], "scheduler_rejected_request")
        recovered = self.invoke(action="recover")
        self.assertEqual(recovered["submission_diagnostic"], result["submission_diagnostic"])
        self.assertEqual(recovered["state"], "UNKNOWN")
        self.assertEqual(self.scheduler.submissions, 1)

    def test_diagnostic_only_exposes_allowlisted_fields(self):
        raw = (b"private secret=never-return-this\\n\n"
               b"sbatch: error: Please submit job from pilogin.hpc.sjtu.edu.cn when using cpu partition.\n")
        result = self.helper.submission_diagnostic(raw)
        self.assertEqual(result["code"], "login_endpoint_mismatch")
        self.assertEqual(result["recommended_cluster"], "pi")
        self.assertNotIn("never-return-this", json.dumps(result))
        unknown = self.helper.submission_diagnostic(b"secret=never-return-this")
        self.assertEqual(unknown["code"], "scheduler_rejected_request")
        self.assertNotIn("never-return-this", json.dumps(unknown))
        self.assertNotIn("required_login_host", self.helper.submission_diagnostic(b"x" * 65537))

    def test_real_command_redacts_stderr_and_returns_scheduler_diagnostic(self):
        helper = load_helper()
        popen = subprocess.Popen
        def scheduler_process(argv, **kwargs):
            return popen([sys.executable, "-c",
                "import sys; sys.stderr.write('private token=SECRET\\n' "
                "+ 'sbatch: error: Please submit job from pilogin.hpc.sjtu.edu.cn when using cpu partition.\\n'); sys.exit(1)"], **kwargs)
        with patch.object(helper.subprocess, "Popen", scheduler_process):
            result = helper.command(["sbatch", "--test-only"])
        self.assertEqual(result["returncode"], 1)
        self.assertEqual(result["diagnostic"]["recommended_cluster"], "pi")
        self.assertNotIn("SECRET", json.dumps(result))
        self.assertNotIn("stderr", result)

    def test_recovery_does_not_validate_a_tampered_frozen_script(self):
        self.scheduler.bad_reply = True
        self.invoke()
        self.scheduler.live = []
        self.scheduler.rows = []
        (Path(self.helper.intent_dir(self.nonce)) / "script.sh").write_text("#!/bin/sh\nfalse\n")
        before = len(self.scheduler.calls)
        result = self.invoke(action="recover")
        self.assertEqual(result["state"], "UNKNOWN")
        self.assertEqual(result["current_validation"]["state"], "UNAVAILABLE")
        self.assertFalse(any(c[0] == "sbatch" for c in self.scheduler.calls[before:]))

    def test_lost_reply_adopted_after_helper_restart(self):
        self.scheduler.lost_reply = True
        self.assertFalse(self.invoke()["ok"])
        restarted = load_helper()
        restarted.command = self.scheduler
        restarted.user = self.helper.user
        restarted.pwd = self.helper.pwd
        request = {"action": "recover", "nonce": self.nonce}
        result = restarted.run(request)
        self.assertTrue(result["adopted"])
        self.assertEqual(result["job_id"], "123")
        self.assertEqual(self.scheduler.submissions, 1)

    def test_uncertain_no_match_never_blindly_retries(self):
        self.scheduler.bad_reply = True
        self.assertEqual(self.invoke()["state"], "UNKNOWN")
        self.scheduler.live = []
        self.scheduler.rows = []
        self.assertEqual(self.invoke()["state"], "UNKNOWN")
        self.assertEqual(self.scheduler.submissions, 1)

    def test_ambiguous_adoption_remains_unknown(self):
        self.scheduler.bad_reply = True
        self.invoke()
        second = list(self.scheduler.rows[0])
        second[0] = "124"
        self.scheduler.rows.append(second)
        result = self.invoke(action="recover")
        self.assertEqual(result["state"], "UNKNOWN")
        self.assertEqual(result["reason"], "ambiguous_matches")

    def test_adoption_requires_exact_name_and_comment(self):
        self.scheduler.bad_reply = True
        self.invoke()
        self.scheduler.live[0][4] += "wrong"
        self.scheduler.rows[0][11] += "wrong"
        result = self.invoke(action="recover")
        self.assertEqual(result["reason"], "no_exact_match")

    def test_commentless_completed_recovery_requires_all_durable_paths(self):
        self.scheduler.bad_reply = True
        self.invoke()
        self.scheduler.live = []
        self.scheduler.rows[0][11] = ""
        result = self.invoke(action="recover")
        self.assertTrue(result["adopted"])
        self.assertEqual(result["job_id"], "123")
        self.assertEqual(self.scheduler.submissions, 1)

    def test_commentless_recovery_rejects_each_wrong_path_and_name(self):
        for field in range(4):
            with self.subTest(field=field):
                # Isolate each uncertain submission's frozen intent.
                self.request["nonce"] = ("%032x" % (field + 1))
                self.scheduler.bad_reply = True
                self.invoke()
                self.scheduler.live = []
                self.scheduler.rows[0][11] = ""
                if field < 3:
                    self.scheduler.output_paths["123"][field] += "wrong"
                else:
                    self.scheduler.rows[0][4] += "wrong"
                self.assertEqual(self.invoke(action="recover")["reason"], "no_exact_match")

    def test_commentless_recovery_rejects_foreign_owner(self):
        self.scheduler.bad_reply = True
        self.invoke()
        self.scheduler.live = []
        self.scheduler.rows[0][11] = ""
        self.scheduler.rows[0][1] = "foreign"
        self.assertFalse(self.invoke(action="recover")["ok"])

    def test_escaped_unicode_large_intent_can_be_reopened(self):
        script = "#!/bin/sh\n#" + "😀" * (180 * 1024)
        self.assertEqual(self.invoke(script=script)["state"], "SUBMITTED")
        self.assertEqual(self.invoke(script=script)["state"], "SUBMITTED")
        self.assertEqual(self.scheduler.submissions, 1)

    def test_tampered_frozen_spec_blocks_recovery(self):
        self.scheduler.bad_reply = True
        self.invoke()
        directory = self.helper.intent_dir(self.nonce)
        record = self.helper.read_json(directory, "intent.json")
        record["spec"]["working_directory"] = "/"
        self.helper.write_json(directory, "intent.json", record)
        self.assertFalse(self.invoke(action="recover")["ok"])

    def test_adoption_with_accounting_unavailable_is_unknown(self):
        self.scheduler.bad_reply = True
        self.invoke()
        self.scheduler.accounting_available = False
        result = self.invoke(action="recover")
        self.assertEqual(result["state"], "UNKNOWN")

    def test_same_nonce_cannot_change_script_or_resources(self):
        self.invoke()
        self.assertFalse(self.invoke(script="#!/bin/sh\nfalse\n")["ok"])
        self.assertFalse(self.invoke(memory_mb=128)["ok"])
        self.assertEqual(self.scheduler.submissions, 1)

    def test_sbatch_only_receives_argv_and_script_filename(self):
        script = "#!/bin/sh\nprintf '%s' '$(touch NEVER)'\n"
        result = self.invoke(script=script, array="0-3%2", gpus=1, account="acct-test")
        self.assertTrue(result["ok"])
        argv = next(c for c in self.scheduler.calls if c[0] == "sbatch")
        self.assertIn("--array=0-3%2", argv)
        self.assertIn("--gpus=1", argv)
        self.assertIn("--account=acct-test", argv)
        self.assertNotIn(script, argv)
        self.assertEqual(Path(argv[-1]).read_text(), script)

    def test_invalid_resource_values_rejected_before_submission(self):
        for field, value in (("partition", "cpu --wrap=evil"), ("account", "-x"),
                             ("memory_mb", True), ("gpus", 0), ("array", "0;evil"),
                             ("dependency", "singleton;evil"), ("working_directory", "/tmp/../study")):
            with self.subTest(field=field):
                result = self.invoke(**{field: value})
                self.assertEqual(result["state"], "REJECTED")
                self.assertFalse(result["submission_attempted"])
                self.assertFalse(Path(self.temp.name, ".autonomics-hpc-jobs").exists())
        self.assertEqual(self.scheduler.submissions, 0)

    def test_invalid_optional_values_never_consume_nonce_or_touch_scheduler(self):
        for field in ("array", "dependency", "qos", "constraint", "reservation"):
            result = self.invoke(**{field: "null"})
            self.assertEqual(result["reason"], "invalid_submission_request")
            self.assertIn('string "null"', result["diagnostic"])
            self.assertFalse(result["submission_attempted"])
            self.assertFalse(Path(self.temp.name, ".autonomics-hpc-jobs").exists())
        self.assertEqual(self.scheduler.calls, [])
        # Corrected input can use the same nonce because validation made no intent.
        self.assertEqual(self.invoke(array=None)["state"], "SUBMITTED")
        self.assertEqual(self.scheduler.submissions, 1)

    def test_preflight_is_pure_and_accepts_omitted_or_json_null_options(self):
        for extra in ({}, {"array": None, "dependency": None}, {"array": "0-3%2"}):
            result = self.invoke(action="validate_submission_request", **extra)
            self.assertEqual(result["state"], "VALIDATED")
            self.assertFalse(result["submission_attempted"])
        self.assertEqual(list(Path(self.temp.name).iterdir()), [])
        self.assertEqual(self.scheduler.calls, [])

    def test_prepared_recovery_requires_the_original_local_request_identity(self):
        self.scheduler.validation_rejected = True
        self.assertEqual(self.invoke()["state"], "REJECTED")
        prepared = self.invoke(action="recover", expected_request=self.request)
        self.assertEqual(prepared["state"], "PREPARED")
        changed = dict(self.request, script="#!/bin/sh\nfalse\n")
        rejected = self.invoke(action="recover", expected_request=changed)
        self.assertFalse(rejected["ok"])
        self.assertIn("expected local submission", rejected["error"])
        self.assertEqual(self.scheduler.submissions, 0)

    def test_unknown_job_is_not_terminal_or_owned(self):
        result = self.invoke(action="status", job_id="123")
        self.assertEqual(result["state"], "UNKNOWN")
        self.assertFalse(result["ownership_verified"])
        self.assertFalse(result["stop_confirmed"])

    def test_owned_queue_reports_pending_reason_without_claiming_stop(self):
        self.invoke()
        self.scheduler.live[0][2] = "PENDING"
        self.scheduler.live[0].append("Resources")
        result = self.invoke(action="status", job_id="123")
        self.assertEqual(result["queue"][0]["pending_reason"], "Resources")
        self.assertFalse(result["stop_confirmed"])
        listed = self.invoke(action="list")
        self.assertEqual(listed["jobs"][0]["reason_or_nodes"], "Resources")
        self.scheduler.live[0][1] = "foreign"
        self.assertFalse(self.invoke(action="list")["ok"])

    def test_zero_queue_with_unavailable_accounting_is_error(self):
        self.scheduler.accounting_available = False
        self.assertFalse(self.invoke(action="status", job_id="123")["ok"])

    def test_completed_job_status_queries_owned_queue_without_jobs_filter(self):
        self.terminal()
        self.scheduler.fail_queue_with_job_filter = True
        result = self.invoke(action="status", job_id="123")
        self.assertTrue(result["ok"])
        self.assertTrue(result["stop_confirmed"])
        self.assertTrue(result["successful"])
        commands = [argv for argv in self.scheduler.calls if argv[0] == "squeue"]
        self.assertTrue(commands)
        self.assertTrue(all(not any(arg.startswith("--jobs=") for arg in argv) for argv in commands))

    def test_target_queue_locally_filters_other_owned_jobs_and_prefixes(self):
        self.terminal()
        self.scheduler.live = [["1234", "toyuser", "RUNNING", "other", "", "toy-node"],
                               ["124", "toyuser", "PENDING", "another", "", "Priority"]]
        result = self.invoke(action="status", job_id="123")
        self.assertTrue(result["stop_confirmed"])
        self.assertEqual(result["queue"], [])
        self.scheduler.live.append(["123_4", "toyuser", "PENDING", "array", "", "Resources"])
        self.assertEqual(self.helper.queue("123"), [{"job_id": "123_4", "user": "toyuser",
            "state": "PENDING", "name": "array", "comment": "", "reason_or_nodes": "Resources",
            "pending_reason": "Resources"}])

    def test_foreign_queue_job_is_rejected_even_when_not_requested_target(self):
        self.terminal()
        self.scheduler.live = [["124", "foreign", "RUNNING", "other", "", "toy-node"]]
        self.assertFalse(self.invoke(action="status", job_id="123")["ok"])

    def test_failed_global_queue_query_is_not_an_empty_queue_proof(self):
        self.terminal()
        self.scheduler.fail_global_queue = True
        self.assertFalse(self.invoke(action="status", job_id="123")["ok"])

    def test_success_requires_all_steps_and_zero_signal(self):
        self.terminal()
        row = list(self.scheduler.rows[0])
        row[0] += ".batch"
        row[3] = "0:9"
        self.scheduler.rows.append(row)
        result = self.invoke(action="status", job_id="123")
        self.assertTrue(result["stop_confirmed"])
        self.assertFalse(result["successful"])
        self.scheduler.rows[-1][3] = "0:0"
        self.assertTrue(self.invoke(action="status", job_id="123")["successful"])

    def test_running_step_prevents_terminal_claim(self):
        self.terminal()
        row = list(self.scheduler.rows[0])
        row[0] += ".batch"
        row[2] = "RUNNING"
        self.scheduler.rows.append(row)
        self.assertFalse(self.invoke(action="status", job_id="123")["stop_confirmed"])

    def test_missing_exit_code_prevents_terminal_claim(self):
        self.terminal(exit_code="Unknown")
        self.assertFalse(self.invoke(action="status", job_id="123")["stop_confirmed"])

    def test_foreign_owner_is_rejected_before_cancel(self):
        self.terminal()
        self.scheduler.rows[0][1] = "someoneelse"
        self.assertFalse(self.invoke(action="cancel", job_id="123")["ok"])
        self.assertFalse(any(c[0] == "scancel" for c in self.scheduler.calls))

    def test_unrelated_prefix_is_not_owned(self):
        self.terminal()
        self.scheduler.rows[0][0] = "1234"
        self.assertFalse(self.invoke(action="status", job_id="123")["ok"])

    def test_array_and_steps_are_in_scope_but_other_array_tasks_are_not(self):
        self.terminal()
        self.scheduler.rows[0][0] = "123_4"
        self.assertTrue(self.invoke(action="status", job_id="123")["stop_confirmed"])
        self.assertFalse(self.invoke(action="status", job_id="123_3")["ok"])

    def test_array_raw_identity_can_differ_from_display_identity(self):
        self.terminal()
        self.scheduler.rows[0][0] = "123_4"
        self.scheduler.raw_ids["123_4"] = "456"
        step = list(self.scheduler.rows[0])
        step[0] = "123_4.batch"
        self.scheduler.rows.append(step)
        self.scheduler.raw_ids["123_4.batch"] = "456.batch"
        result = self.invoke(action="status", job_id="123_4")
        self.assertTrue(result["stop_confirmed"])
        self.assertEqual(result["accounting"][0]["job_id_raw"], "456")
        self.scheduler.raw_ids["123_4.batch"] = "789.batch"
        self.assertFalse(self.invoke(action="status", job_id="123_4")["ok"])

    def test_adoption_of_array_collapses_tasks_to_the_exact_parent(self):
        self.scheduler.bad_reply = True
        self.invoke()
        self.scheduler.live = []
        first = self.scheduler.rows[0]
        first[0] = "123_0"
        second = list(first)
        second[0] = "123_1"
        self.scheduler.rows.append(second)
        self.scheduler.raw_ids = {"123_0": "456", "123_1": "457"}
        result = self.invoke(action="recover")
        self.assertEqual(result["job_id"], "123")
        self.assertTrue(result["adopted"])

    def test_cancel_confirms_terminal_accounting(self):
        self.invoke()
        result = self.invoke(action="cancel", job_id="123")
        self.assertTrue(result["stop_confirmed"])
        self.assertEqual(result["state"], "STOP_CONFIRMED")

    def test_cancel_does_not_claim_stop_after_request_only(self):
        self.invoke()
        self.scheduler.stuck_cancel = True
        with patch.object(self.helper.time, "sleep"):
            result = self.invoke(action="cancel", job_id="123")
        self.assertTrue(result["cancel_requested"])
        self.assertFalse(result["stop_confirmed"])
        self.assertEqual(result["state"], "STOP_PENDING")

    def test_unknown_job_cannot_be_cancelled(self):
        self.assertFalse(self.invoke(action="cancel", job_id="123")["ok"])
        self.assertFalse(any(c[0] == "scancel" for c in self.scheduler.calls))

    def test_hold_release_requeue_and_update_only_owned_job(self):
        self.invoke()
        for action in ("hold", "release", "requeue", "update"):
            result = self.invoke(action=action, job_id="123")
            self.assertTrue(result["request_accepted"])
        self.assertIn(["scontrol", "update", "JobId=123", "TimeLimit=00:01:00"], self.scheduler.calls)

    def test_list_can_include_bounded_accounting_history(self):
        self.terminal()
        row = list(self.scheduler.rows[0])
        row[0] = "124"
        self.scheduler.rows.append(row)
        result = self.invoke(action="list", include_completed=True,
                             start_time="2026-10-01", limit=1)
        self.assertEqual(len(result["history"]["allocations"]), 1)
        self.assertTrue(result["history"]["truncated"])
        self.assertEqual(result["history"]["total_returned_by_scheduler"], 2)
        self.assertFalse(self.invoke(action="list", include_completed=True,
                                     start_time="now; command")["ok"])

    def test_history_rejects_other_account(self):
        self.terminal()
        self.scheduler.rows[0][1] = "foreign"
        self.assertFalse(self.invoke(action="list", include_completed=True)["ok"])

    def test_successful_quota_command_does_not_invent_verified_limits(self):
        result = self.helper.run({"tool": "cluster", "action": "quota", "path": self.temp.name})
        self.assertTrue(result["ok"])
        self.assertFalse(result["quota_verified"])
        self.assertEqual(result["qualification"], "RAW_PROVIDER_OUTPUT_ONLY")
        self.assertNotIn("filesystem", result["successful_quota_queries"])

    def test_logs_are_bounded_and_byte_hash_is_returned(self):
        self.terminal()
        logfile = Path(self.temp.name) / "log with space.txt"
        logfile.write_bytes(b"abcdef")
        self.scheduler.logs = "JobId=123 StdOut=" + str(logfile) + " StdErr=/unused WorkDir=/\n"
        result = self.invoke(action="logs", job_id="123", offset=1, max_bytes=3)
        self.assertEqual(result["content"], "bcd")
        self.assertEqual(result["next_offset"], 4)
        self.assertTrue(result["truncated"])
        self.assertEqual(len(result["sha256"]), 64)

    def test_log_symlink_and_fifo_are_rejected(self):
        self.terminal()
        logfile = Path(self.temp.name) / "file"
        logfile.write_bytes(b"hello")
        link = Path(self.temp.name) / "link"
        link.symlink_to(logfile)
        fifo = Path(self.temp.name) / "fifo"
        os.mkfifo(fifo)
        for candidate in (link, fifo):
            self.assertFalse(self.invoke(action="logs", job_id="123", path=str(candidate))["ok"])

    def test_private_state_symlink_is_rejected(self):
        root = Path(self.temp.name) / ".autonomics-hpc-jobs"
        root.symlink_to(self.temp.name)
        self.assertFalse(self.invoke()["ok"])

    def test_recover_missing_intent_does_not_submit(self):
        result = self.invoke(action="recover")
        self.assertEqual(result["state"], "UNKNOWN")
        self.assertEqual(result["reason"], "remote_intent_missing")
        self.assertFalse(result["new_job_submitted"])
        self.assertFalse(result["automatic_resubmission_permitted"])
        self.assertEqual(list(Path(self.temp.name).iterdir()), [])
        self.assertEqual(self.scheduler.calls, [])
        self.assertEqual(self.scheduler.submissions, 0)

    def test_recover_legacy_lock_only_keeps_evidence_without_fabricating_intent(self):
        directory = Path(self.helper.intent_dir(self.nonce))
        lock = directory / "lock"
        lock.write_bytes(b"original lock evidence")
        lock.chmod(0o600)
        before = {p.name: (p.read_bytes(), p.stat().st_mtime_ns) for p in directory.iterdir()}
        result = self.invoke(action="recover")
        self.assertEqual(result["state"], "UNKNOWN")
        self.assertEqual(result["reason"], "remote_intent_missing")
        self.assertEqual(before, {p.name: (p.read_bytes(), p.stat().st_mtime_ns) for p in directory.iterdir()})
        self.assertEqual(self.scheduler.calls, [])

    def test_recover_missing_nonce_does_not_create_it_under_existing_root(self):
        self.helper.intent_dir("f" * 32)
        root = Path(self.temp.name, ".autonomics-hpc-jobs")
        result = self.invoke(action="recover")
        self.assertEqual(result["reason"], "remote_intent_missing")
        self.assertEqual([p.name for p in root.iterdir()], ["f" * 32])
        self.assertEqual(self.scheduler.calls, [])

    def test_recover_missing_lock_preserves_intent_and_does_not_recreate_lock(self):
        self.invoke()
        directory = Path(self.helper.intent_dir(self.nonce))
        (directory / "lock").unlink()
        before = (directory / "intent.json").read_bytes()
        self.scheduler.calls.clear()
        result = self.invoke(action="recover")
        self.assertEqual(result["state"], "UNKNOWN")
        self.assertEqual(result["reason"], "remote_submission_lock_missing")
        self.assertFalse((directory / "lock").exists())
        self.assertEqual((directory / "intent.json").read_bytes(), before)
        self.assertEqual(self.scheduler.calls, [])

    def test_old_stale_temporary_record_does_not_block_recovery(self):
        directory = Path(self.helper.intent_dir(self.nonce))
        (directory / ".intent.json.tmp").write_text("interrupted")
        self.assertEqual(self.invoke()["state"], "SUBMITTED")

    def test_prepared_intent_with_interrupted_script_is_safe_to_finish(self):
        directory = self.helper.intent_dir(self.nonce)
        spec = self.helper.normalize_spec(self.request)
        digest = self.helper.hashlib.sha256(json.dumps(spec, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        self.helper.write_json(directory, "intent.json", {"version": 1, "nonce": self.nonce,
            "owner_uid": os.getuid(), "state": "PREPARED", "spec": spec,
            "request_sha256": digest, "created_at": "2026-10-24T00:00:00"})
        Path(directory, "script.sh").write_text("partial")
        self.assertEqual(self.invoke()["state"], "SUBMITTED")

    def test_helpers_are_python36_syntax_compatible(self):
        import ast
        ast.parse(HELPER.read_text(), feature_version=(3, 6))


if __name__ == "__main__":
    unittest.main()
