"""Quota protocol tests. No scheduler, SSH, filesystem quota mutations."""
import importlib.util
import os
from pathlib import Path
import types
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[1] / 'src' / 'remote_jobs.py'
HEADER = 'filesystemName quotaType id name blockUsage blockQuota blockLimit blockInDoubt blockGrace filesUsage filesQuota filesLimit filesInDoubt filesGrace remarks fid filesetname'.split()


def record(scope, identity, **changes):
    row = dict(zip(HEADER, ['dssg01', {'user':'USR','group':'GRP','fileset':'FILESET'}[scope],
        str(identity), 'acct-test' if scope == 'fileset' else str(identity),
        '100', '0', '0', '5', 'none', '30', '0', '0', '2', 'none', 'disk', '7', 'acct-test']))
    if scope == 'fileset':
        row.update(blockLimit='1000', filesLimit='100')
    row.update({k: str(v) for k, v in changes.items()})
    prefix = 'mmlsquota:' + scope
    return {'returncode':0, 'available':True, 'stdout':
        prefix + ':HEADER:version:reserved:reserved:' + ':'.join(HEADER) + ':\n' +
        prefix + ':0:1:::' + ':'.join(row[k] for k in HEADER) + ':\n'}


class QuotaTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location('quota_helper', SOURCE)
        self.h = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.h)

    def test_gpfs_client_discovery_without_interactive_path(self):
        with patch.object(self.h.shutil, 'which', return_value=None), patch.object(self.h.os.path, 'isfile', return_value=True), patch.object(self.h.os, 'access', return_value=True):
            self.assertEqual(self.h.cluster_executable('mmlsattr'), '/usr/lpp/mmfs/bin/mmlsattr')
            self.assertEqual(self.h.cluster_executable('mmlsquota'), '/usr/lpp/mmfs/bin/mmlsquota')
            self.assertIsNone(self.h.cluster_executable('unapproved-tool'))

    def test_unlimited_user_does_not_mean_unlimited_fileset(self):
        user = self.h.gpfs_record(record('user', 1), 'user', 1, 'acct-test')
        group = self.h.gpfs_record(record('fileset', 7), 'fileset', 'acct-test', 'acct-test')
        self.assertIsNone(user['bytes']['remaining_conservative'])
        self.assertEqual(group['bytes']['remaining_conservative'], 895 * 1024)
        self.assertEqual(group['files']['remaining_conservative'], 68)

    def test_soft_limit_and_in_doubt_are_conservative(self):
        value = self.h.gpfs_record(record('user', 1, blockQuota=200, blockLimit=1000), 'user', 1, 'acct-test')
        self.assertEqual(value['bytes']['remaining_conservative'], 95 * 1024)

    def test_exhaustion_is_verified_zero_not_unknown(self):
        value = self.h.gpfs_record(record('user', 1, blockLimit=50), 'user', 1, 'acct-test')
        self.assertEqual(value['bytes']['remaining_conservative'], 0)

    def test_malformed_or_other_scope_never_verified(self):
        for changes in ({'id':99}, {'filesetname':'other'}, {'blockUsage':'unknown'}, {'blockLimit':'-1'}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.h.gpfs_record(record('user', 1, **changes), 'user', 1, 'acct-test')

    def test_empty_success_and_duplicate_records_rejected(self):
        value = record('user', 1)
        for raw in ('', value['stdout'] + value['stdout'].splitlines()[1] + '\n'):
            with self.assertRaises(ValueError):
                self.h.gpfs_record(dict(value, stdout=raw), 'user', 1, 'acct-test')

    def snapshot(self, fail_group=False):
        calls = []
        def command(argv, **kw):
            calls.append(argv)
            if argv[0] == 'df':
                return {'returncode':0, 'stdout':'Filesystem Type 1024-blocks Used Available Capacity Mounted on\nrdssg01 gpfs 9000 1000 8000 12% /dssg\n'}
            if argv[0] == 'mmlsattr':
                return {'returncode':0, 'stdout':'fileset name:         acct-test\n'}
            scope = 'user' if '-u' in argv else 'group' if '-g' in argv else 'fileset'
            if fail_group and scope == 'group':
                return {'returncode':1, 'stdout':''}
            identity = os.getuid() if scope == 'user' else argv[argv.index('-g')+1] if scope == 'group' else 7
            return record(scope, identity)
        with patch.object(self.h, 'command', command), patch.object(self.h.os, 'stat', return_value=types.SimpleNamespace(st_gid=os.getgid())):
            result = self.h.quota_snapshot({'path':'/dssg/home/acct-test/test'})
        return result, calls

    def test_actual_device_fileset_scope_and_units(self):
        result, calls = self.snapshot()
        self.assertTrue(result['quota_verified'])
        self.assertEqual(result['quota_remaining_bytes'], 895*1024)
        self.assertEqual(result['quota_remaining_files'], 68)
        self.assertFalse(result['reservation'])
        for argv in calls:
            if argv[0] == 'mmlsquota':
                self.assertIn(argv[-1], ('rdssg01', 'rdssg01:acct-test'))
        self.assertFalse(any(argv[0] == 'quota' for argv in calls))

    def test_group_permission_failure_is_unknown(self):
        result, _ = self.snapshot(True)
        self.assertFalse(result['quota_verified'])
        self.assertNotIn('available_bytes_conservative', result)
        self.assertIn('group', result['reason'])

    def test_optional_provider_timeout_keeps_explicit_unknown(self):
        with patch.object(self.h, 'command', side_effect=ValueError('timeout')):
            result = self.h.quota_snapshot({'path':'/dssg/test'})
        self.assertFalse(result['quota_verified'])
        self.assertEqual(result['queries']['filesystem']['diagnostic']['code'], 'quota_query_incomplete')


if __name__ == '__main__':
    unittest.main()
