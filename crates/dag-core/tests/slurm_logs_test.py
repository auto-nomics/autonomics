"""Self-cleaning toy log fixtures; no SSH, scheduler or study data."""
import hashlib
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

source = Path(__file__).parents[1] / 'src/dag/execution/slurm_logs.py'
spec = importlib.util.spec_from_file_location('slurm_logs', source)
logs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(logs)


class LogSnapshots(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.directory = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY)

    def tearDown(self):
        os.close(self.directory)
        self.temp.cleanup()

    def capture(self):
        return logs.capture(str(self.root), self.directory, '42', 'stderr', 'err')

    def test_missing_log_is_explicitly_absent(self):
        self.assertIsNone(self.capture())

    def test_small_log_roundtrips_and_repeat_read_agrees(self):
        data = b'Error: input missing\n'
        (self.root / 'slurm-42.err').write_bytes(data)
        reference = self.capture()
        self.assertEqual(Path(reference['path']).read_bytes(), data)
        self.assertEqual(reference['fingerprint']['content_hash'], hashlib.sha256(data).hexdigest())
        self.assertEqual(reference, self.capture())
        self.assertEqual(Path(reference['path']).stat().st_mode & 0o777, 0o600)

    def test_large_log_retains_bounded_tail_and_raw_log(self):
        data = b'x' * (logs.LIMIT * 3) + b'last diagnostic\n'
        raw = self.root / 'slurm-42.err'
        raw.write_bytes(data)
        reference = self.capture()
        snapshot = Path(reference['path']).read_bytes()
        self.assertIn(b'truncated', snapshot[:60])
        self.assertTrue(snapshot.endswith(b'last diagnostic\n'))
        self.assertLess(len(snapshot), logs.LIMIT + 100)
        self.assertEqual(raw.read_bytes(), data)

    def test_symlink_source_is_rejected(self):
        (self.root / 'other').write_bytes(b'should not read')
        (self.root / 'slurm-42.err').symlink_to('other')
        with self.assertRaises(OSError):
            self.capture()

    def test_symlink_snapshot_is_rejected(self):
        (self.root / 'slurm-42.err').write_bytes(b'error')
        (self.root / 'other').write_bytes(b'should not replace')
        (self.root / 'transport-stderr.log').symlink_to('other')
        with self.assertRaises(OSError):
            self.capture()
        self.assertEqual((self.root / 'other').read_bytes(), b'should not replace')

    def test_changed_log_cannot_replace_old_snapshot(self):
        raw = self.root / 'slurm-42.err'
        raw.write_bytes(b'original')
        self.capture()
        raw.write_bytes(b'replacement')
        with self.assertRaises(ValueError):
            self.capture()
        self.assertEqual((self.root / 'transport-stderr.log').read_bytes(), b'original')


if __name__ == '__main__':
    unittest.main()
