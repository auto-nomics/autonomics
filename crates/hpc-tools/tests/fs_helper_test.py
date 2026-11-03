"""Self-cleaning account-filesystem toy fixtures; no cluster or study data."""
import base64
import ctypes
import errno
import hashlib
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

HELPER_PATH = Path(__file__).resolve().parents[1] / "src" / "remote_fs.py"
spec = importlib.util.spec_from_file_location("hpc_fs_helper", str(HELPER_PATH))
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class FilesystemTests(unittest.TestCase):
    def test_quoted_null_is_an_argument_error_before_any_file_is_created(self):
        for value in ("null", "", "not-a-sha256"):
            with self.subTest(value=value), mock.patch.object(helper.tempfile, "mkstemp", side_effect=AssertionError("must validate before creating scratch")):
                result = self.call("write", content="new", expected_sha256=value)
                self.assertFalse(result["ok"])
                self.assertIn("expected_sha256", result["message"])
                self.assertIn("JSON null", result["message"])
        self.assertEqual(list(self.root.iterdir()), [])
        self.assertTrue(self.call("write", content="null", expected_sha256=None)["ok"])
        self.assertEqual((self.root / "toy").read_text(), "null")

    def test_upload_requires_its_original_path_and_keeps_staging_on_bad_abort(self):
        begin = self.call("upload_begin")
        handle = begin["upload_id"]
        for value in (None, "", "null"):
            with self.subTest(path=value):
                result = helper.run(dict(action="upload_abort", path=value, upload_id=handle))
                self.assertFalse(result["ok"])
                self.assertIn("path", result["message"])
                self.assertNotEqual(result["error"], "FileNotFoundError")
        self.assertTrue(self.call("upload_status", upload_id=handle)["ok"])
        self.assertTrue(self.call("upload_abort", upload_id=handle)["aborted"])
        self.assertEqual(list(self.root.iterdir()), [])

    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="hpc-fs-toy-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)

    def call(self, action, filename="toy", **kwargs):
        return helper.run(dict(action=action, path=str(self.root / filename), **kwargs))

    def test_no_clobber_and_explicit_replacement(self):
        first = self.call("write", content="old")
        self.assertTrue(first["ok"])
        self.assertFalse(self.call("write", content="new")["ok"])
        self.assertFalse(self.call("write", content="new", expected_sha256="0" * 64)["ok"])
        self.assertEqual((self.root / "toy").read_bytes(), b"old")
        replaced = self.call("write", content="new", expected_sha256=first["sha256"])
        self.assertTrue(replaced["ok"])
        self.assertEqual((self.root / "toy").read_bytes(), b"new")
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ["toy"])

    def test_string_null_is_a_parameter_error_not_a_filesystem_outage(self):
        for field in ("expected_sha256", "encoding", "upload_id"):
            rejected = self.call("write", content="null", **{field: "null"})
            self.assertFalse(rejected["ok"])
            self.assertIn(field, rejected["message"])
            self.assertIn('string "null"', rejected["message"])
            self.assertEqual(list(self.root.iterdir()), [])
        created = self.call("write", content="null", expected_sha256=None)
        self.assertTrue(created["ok"])
        self.assertEqual((self.root / "toy").read_text(), "null")
        self.assertFalse(self.call("write", content="changed", expected_sha256="null")["ok"])
        self.assertEqual((self.root / "toy").read_text(), "null")

    def test_upload_abort_requires_original_path_and_keeps_staging_on_bad_input(self):
        handle = self.call("upload_begin")["upload_id"]
        rejected = helper.run({"action": "upload_abort", "path": "null", "upload_id": handle})
        self.assertFalse(rejected["ok"])
        self.assertIn('string "null"', rejected["message"])
        self.assertTrue(self.call("upload_status", upload_id=handle)["ok"])
        self.assertTrue(self.call("upload_abort", upload_id=handle)["aborted"])

    def test_wrong_upload_directory_is_unresolved_and_does_not_discard_original_staging(self):
        begin = self.call("upload_begin")
        handle = begin["upload_id"]
        (self.root / "other").mkdir()
        for action in ("upload_status", "upload_abort"):
            rejected = helper.run({"action": action, "path": str(self.root / "other" / "toy"), "upload_id": handle})
            self.assertFalse(rejected["ok"])
            self.assertEqual(rejected["reason"], "upload_state_not_found_at_requested_path")
            self.assertTrue(rejected["upload_state_unresolved"])
            self.assertIn("exact full path", rejected["next_action"])
            self.assertEqual(self.call("upload_status", upload_id=handle)["size"], 0)
        self.assertTrue(self.call("upload_abort", upload_id=handle)["aborted"])

    def test_binary_range_hash_and_endpoint_metadata(self):
        data = b"\x00\xffhello\x80"
        (self.root / "toy").write_bytes(data)
        result = self.call("read", offset=1, limit=3, encoding="base64")
        self.assertEqual(base64.b64decode(result["content"]), data[1:4])
        self.assertEqual(result["chunk_sha256"], hashlib.sha256(data[1:4]).hexdigest())
        self.assertEqual(result["next_offset"], 4)
        self.assertFalse(result["eof"])
        self.assertIn("account_home", result)
        self.assertIn("hostname", result)

    def test_symlink_and_fifo_are_not_read_or_overwritten(self):
        (self.root / "real").write_bytes(b"private")
        (self.root / "toy").symlink_to("real")
        self.assertFalse(self.call("read")["ok"])
        self.assertFalse(self.call("write", content="bad")["ok"])
        self.assertEqual((self.root / "real").read_bytes(), b"private")
        self.call("remove")
        self.assertTrue((self.root / "real").exists())
        os.mkfifo(str(self.root / "fifo"))
        self.assertFalse(self.call("read", "fifo")["ok"])

    def test_upload_chunk_retry_and_verified_atomic_commit(self):
        begin = self.call("upload_begin")
        handle = begin["upload_id"]
        data = b"first\x00second"
        sha = hashlib.sha256(data).hexdigest()
        args = dict(upload_id=handle, content=base64.b64encode(data).decode(), encoding="base64", sha256=sha, offset=0)
        stored = self.call("upload_chunk", **args)
        self.assertEqual(stored["offset"], len(data))
        self.assertTrue(self.call("upload_chunk", **args)["replayed"])
        self.assertFalse((self.root / "toy").exists())
        self.assertFalse(self.call("upload_commit", upload_id=handle, size=len(data), sha256="0" * 64)["ok"])
        committed = self.call("upload_commit", upload_id=handle, size=len(data), sha256=sha)
        self.assertTrue(committed["committed"])
        self.assertEqual((self.root / "toy").read_bytes(), data)
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ["toy"])

    def test_upload_rejects_bad_offsets_chunks_and_rebinding(self):
        handle = self.call("upload_begin")["upload_id"]
        self.assertFalse(self.call("upload_chunk", upload_id=handle, content="one", offset=1, sha256=hashlib.sha256(b"one").hexdigest())["ok"])
        self.assertFalse(self.call("upload_chunk", upload_id=handle, content="one", offset=0, sha256="0" * 64)["ok"])
        self.assertFalse(self.call("upload_commit", "other", upload_id=handle, size=0, sha256=hashlib.sha256(b"").hexdigest())["ok"])
        self.assertTrue(self.call("upload_abort", upload_id=handle)["aborted"])
        self.assertEqual(list(self.root.iterdir()), [])

    def test_upload_status_returns_validated_resume_offset_without_contents(self):
        handle = self.call("upload_begin")["upload_id"]
        status = self.call("upload_status", upload_id=handle)
        self.assertTrue(status["ok"])
        self.assertEqual(status["offset"], 0)
        self.assertNotIn("content", status)
        data = b"resume toy"
        self.call("upload_chunk", upload_id=handle, content=data.decode(), offset=0, sha256=hashlib.sha256(data).hexdigest())
        resumed = self.call("upload_status", upload_id=handle)
        self.assertEqual(resumed["offset"], len(data))
        self.assertEqual(resumed["staging_identity"][:2], status["staging_identity"][:2])
        self.assertFalse(self.call("upload_status", "other", upload_id=handle)["ok"])

    def test_lost_commit_ack_only_verifies_current_bytes_not_historical_success(self):
        handle = self.call("upload_begin")["upload_id"]
        data = b"current desired bytes"
        sha = hashlib.sha256(data).hexdigest()
        self.call("upload_chunk", upload_id=handle, content=data.decode(), offset=0, sha256=sha)
        self.assertTrue(self.call("upload_commit", upload_id=handle, size=len(data), sha256=sha)["committed"])
        retry = self.call("upload_commit", upload_id=handle, size=len(data), sha256=sha)
        self.assertTrue(retry["current_target_bytes_verified"])
        self.assertFalse(retry["committed"])
        self.assertFalse(retry["historical_commit_verified"])
        self.assertFalse(retry["receipt_authenticity_verified"])
        self.assertFalse(self.call("upload_commit", upload_id=handle, size=len(data), sha256="0" * 64)["ok"])

    def test_upload_staging_inode_replacement_rejected(self):
        handle = self.call("upload_begin")["upload_id"]
        part = self.root / (".autonomics-upload-" + handle + ".part")
        replacement = self.root / "replacement"
        replacement.write_bytes(b"bad")
        replacement.chmod(0o600)
        os.replace(str(replacement), str(part))
        result = self.call("upload_commit", upload_id=handle, size=3, sha256=hashlib.sha256(b"bad").hexdigest())
        self.assertFalse(result["ok"])
        self.assertFalse((self.root / "toy").exists())

    def test_replacement_between_metadata_read_and_part_open_is_rejected(self):
        for action in ("upload_status", "upload_chunk", "upload_commit", "upload_abort"):
            with self.subTest(action=action):
                handle = self.call("upload_begin")["upload_id"]
                part = self.root / (".autonomics-upload-" + handle + ".part")
                metadata = self.root / (".autonomics-upload-" + handle + ".json")
                original = helper.regular_fd
                swapped = []

                def replace_then_open(target, write=False):
                    if str(target) == str(part) and write and not swapped:
                        replacement = self.root / "replacement"
                        replacement.write_bytes(b"unrelated")
                        replacement.chmod(0o600)
                        os.replace(str(replacement), str(part))
                        swapped.append(True)
                    return original(target, write)

                with mock.patch.object(helper, "regular_fd", replace_then_open):
                    result = self.call(action, upload_id=handle, offset=0, content="append",
                                       size=9, sha256=hashlib.sha256(b"unrelated").hexdigest())
                self.assertTrue(swapped)
                self.assertFalse(result["ok"], result)
                self.assertEqual(part.read_bytes(), b"unrelated")
                self.assertTrue(metadata.exists())
                self.assertFalse((self.root / "toy").exists())

    def test_abort_holds_staging_lock_before_removing_either_named_file(self):
        handle = self.call("upload_begin")["upload_id"]
        locks = []
        removals = []
        original_lock = helper.fcntl.flock
        original_unlink = helper.os.unlink

        def observe_lock(fd, operation):
            result = original_lock(fd, operation)
            if operation == helper.fcntl.LOCK_EX:
                locks.append(fd)
            return result

        def observe_unlink(target, *args, **kwargs):
            self.assertTrue(locks, "abort removed a file before locking its staging inode")
            self.assertTrue(os.fstat(locks[-1]))
            removals.append(str(target))
            return original_unlink(target, *args, **kwargs)

        with mock.patch.object(helper.fcntl, "flock", observe_lock), mock.patch.object(helper.os, "unlink", observe_unlink):
            result = self.call("upload_abort", upload_id=handle)
        self.assertTrue(result["aborted"])
        self.assertEqual(len(removals), 2)

    def test_metadata_change_while_waiting_for_part_lock_is_rejected(self):
        handle = self.call("upload_begin")["upload_id"]
        metadata = self.root / (".autonomics-upload-" + handle + ".json")
        original = helper.fcntl.flock

        def change_metadata_after_lock(fd, operation):
            result = original(fd, operation)
            record = __import__("json").loads(metadata.read_text())
            record["expected_sha256"] = "0" * 64
            metadata.write_text(__import__("json").dumps(record))
            return result

        with mock.patch.object(helper.fcntl, "flock", change_metadata_after_lock):
            result = self.call("upload_abort", upload_id=handle)
        self.assertFalse(result["ok"])
        self.assertTrue(metadata.exists())
        self.assertTrue((self.root / (".autonomics-upload-" + handle + ".part")).exists())

    def test_search_cursor_covers_punctuation_and_nested_paths(self):
        (self.root / "a").mkdir()
        (self.root / "a" / "inside").touch()
        (self.root / "a.b").touch()
        expected = {str(self.root / "a"), str(self.root / "a" / "inside"), str(self.root / "a.b")}
        seen = set()
        cursor = None
        for _ in range(5):
            result = self.call("search", "", recursive=True, limit=1, cursor=cursor)
            self.assertTrue(result["ok"], result)
            seen.update(item["path"] for item in result["matches"])
            cursor = result["cursor"]
            if cursor is None:
                break
        self.assertEqual(seen, expected)

    def test_listing_pages_and_shell_metacharacters_are_literal(self):
        for name in ["$(touch NEVER)", "a'b", "line\nname"]:
            self.assertTrue(self.call("write", name, content="literal")["ok"])
        seen = set()
        page = 0
        for _ in range(5):
            result = self.call("list", "", page=page, limit=1)
            seen.update(row["name"] for row in result["entries"])
            page = result["next_page"]
            if page is None:
                break
        self.assertEqual(seen, {"$(touch NEVER)", "a'b", "line\nname"})
        self.assertFalse((self.root / "NEVER").exists())

    def test_copy_move_and_recursive_removal_are_explicit(self):
        self.call("write", content="original")
        copy = self.call("copy", destination=str(self.root / "copy"))
        self.assertTrue(copy["ok"])
        self.assertFalse(self.call("copy", destination=str(self.root / "copy"))["ok"])
        moved = self.call("move", "copy", destination=str(self.root / "moved"))
        self.assertTrue(moved["moved"])
        self.assertFalse((self.root / "copy").exists())
        self.call("mkdir", "dir")
        self.call("write", "dir/nested", content="toy")
        self.assertFalse(self.call("remove", "dir")["ok"])
        self.assertTrue(self.call("remove", "dir", recursive=True)["ok"])

    def test_directory_copy_and_move_cannot_overwrite_existing_paths(self):
        self.call("mkdir", "tree")
        self.call("write", "tree/leaf", content="tree bytes")
        self.assertFalse(self.call("copy", "tree", destination=str(self.root / "copied"))["ok"])
        # GPFS/Lustre may reject kernel no-replace directory rename. This case
        # checks recursive copy/move; strict refusal is covered independently.
        copied = self.call("copy", "tree", destination=str(self.root / "copied"), recursive=True,
                           directory_publish_mode="cooperative_reservation")
        self.assertTrue(copied["ok"], copied)
        self.assertEqual((self.root / "copied" / "leaf").read_text(), "tree bytes")
        self.assertFalse(self.call("move", "tree", destination=str(self.root / "copied"))["ok"])
        moved = self.call("move", "tree", destination=str(self.root / "moved"),
                          directory_publish_mode="cooperative_reservation")
        self.assertTrue(moved["ok"], moved)
        self.assertFalse((self.root / "tree").exists())
        self.assertFalse(self.call("copy", "moved", destination=str(self.root / "moved" / "inside"), recursive=True)["ok"])

    def test_explicit_cooperative_directory_copy_move_and_flags(self):
        self.call("mkdir", "tree")
        self.call("write", "tree/leaf", content="toy directory bytes")
        copied = self.call("copy", "tree", destination=str(self.root / "copied"), recursive=True,
                           directory_publish_mode="cooperative_reservation")
        self.assertTrue(copied["ok"], copied)
        self.assertFalse(copied["atomic_noreplace"])
        self.assertTrue(copied["cooperative_guard"])
        self.assertFalse(copied["external_same_uid_cas"])
        self.assertEqual((self.root / "copied" / "leaf").read_text(), "toy directory bytes")
        moved = self.call("move", "copied", destination=str(self.root / "moved"),
                          directory_publish_mode="cooperative_reservation")
        self.assertTrue(moved["ok"], moved)
        self.assertFalse((self.root / "copied").exists())
        self.assertEqual((self.root / "moved" / "leaf").read_text(), "toy directory bytes")
        self.assertEqual((self.root / ".autonomics-directory-publish.lock").stat().st_mode & 0o777, 0o600)

    def test_cooperative_mode_rejects_existing_empty_directory_file_and_symlink(self):
        self.call("mkdir", "source")
        self.call("write", "source/leaf", content="retained")
        self.call("mkdir", "existing-dir")
        self.call("write", "existing-file", content="existing bytes")
        (self.root / "existing-link").symlink_to("source")
        for name in ("existing-dir", "existing-file", "existing-link"):
            with self.subTest(name=name):
                before = os.lstat(str(self.root / name))
                result = self.call("move", "source", destination=str(self.root / name),
                                   directory_publish_mode="cooperative_reservation")
                self.assertFalse(result["ok"])
                after = os.lstat(str(self.root / name))
                self.assertEqual((before.st_dev, before.st_ino), (after.st_dev, after.st_ino))
                self.assertEqual((self.root / "source" / "leaf").read_text(), "retained")

    def test_strict_unsupported_rename_never_silently_uses_cooperative_mode(self):
        self.call("mkdir", "source")
        self.call("write", "source/leaf", content="strict source")

        def unsupported(*args):
            ctypes.set_errno(errno.EINVAL)
            return -1

        with mock.patch.object(helper.ctypes, "CDLL") as library:
            library.return_value.renameat2.side_effect = unsupported
            result = self.call("move", "source", destination=str(self.root / "destination"))
        self.assertFalse(result["ok"])
        self.assertFalse((self.root / "destination").exists())
        self.assertFalse((self.root / ".autonomics-directory-publish.lock").exists())
        self.assertEqual((self.root / "source" / "leaf").read_text(), "strict source")

    def test_cooperative_rename_failure_retains_reservation_without_unlink_race(self):
        self.call("mkdir", "source")
        self.call("write", "source/leaf", content="retained source")
        with mock.patch.object(helper.os, "rename", side_effect=OSError(errno.EIO, "toy injected rename failure")):
            result = self.call("move", "source", destination=str(self.root / "destination"),
                               directory_publish_mode="cooperative_reservation")
        self.assertFalse(result["ok"])
        self.assertTrue(result["reservation_retained"])
        self.assertFalse(result["reservation_cleanup_attempted"])
        self.assertTrue(result["mutation_may_have_completed"])
        self.assertEqual(list((self.root / "destination").iterdir()), [])
        self.assertEqual((self.root / "source" / "leaf").read_text(), "retained source")

    def test_replaced_cooperative_reservation_is_preserved_and_never_published_over(self):
        self.call("mkdir", "source")
        self.call("write", "source/leaf", content="original")
        destination = self.root / "destination"
        source = str(self.root / "source")
        original_stat = helper.os.stat
        swapped = []

        def replace_after_reservation(target, *args, **kwargs):
            if str(target) == source and destination.exists() and not swapped:
                os.rmdir(str(destination))
                os.mkdir(str(destination), 0o700)
                (destination / "foreign-marker").write_text("preserve these bytes")
                swapped.append(True)
            return original_stat(target, *args, **kwargs)

        with mock.patch.object(helper.os, "stat", replace_after_reservation):
            result = self.call("move", "source", destination=str(destination),
                               directory_publish_mode="cooperative_reservation")
        self.assertTrue(swapped)
        self.assertFalse(result["ok"])
        self.assertFalse(result["reservation_retained"])
        self.assertFalse(result["reservation_cleanup_attempted"])
        self.assertEqual((destination / "foreign-marker").read_text(), "preserve these bytes")
        self.assertEqual((self.root / "source" / "leaf").read_text(), "original")

    def test_limits_and_binary_errors_do_not_echo_file_bodies(self):
        (self.root / "toy").write_bytes(b"\xffSECRET")
        result = self.call("read")
        self.assertFalse(result["ok"])
        self.assertNotIn("SECRET", str(result))
        self.assertFalse(self.call("read", limit=helper.CHUNK + 1)["ok"])
        self.assertFalse(self.call("read", offset=True)["ok"])


if __name__ == "__main__":
    unittest.main()
