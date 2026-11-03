"""Read bounded advisory telemetry for one exact submitted attempt, never a DB."""
import json
import os
import stat
import sys


def read(path, task_id, job_id, manifest_sha256):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        before = os.fstat(descriptor)
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                or before.st_nlink != 1 or before.st_size > 4096):
            return None
        raw = os.read(descriptor, 4097)
        after = os.fstat(descriptor)
        if (len(raw) != before.st_size or before.st_size != after.st_size
                or before.st_mtime_ns != after.st_mtime_ns):
            return None
    finally:
        os.close(descriptor)
    value = json.loads(raw)
    if not isinstance(value, dict):
        return None
    if (value.get('schema') != 'AUTONOMICS_WORKER_HEARTBEAT_V1'
            or value.get('task_id') != task_id or value.get('job_id') != job_id
            or value.get('manifest_sha256') != manifest_sha256
            or value.get('state') not in ('active', 'exiting')
            or type(value.get('pid')) is not int or value['pid'] <= 0
            or type(value.get('sequence')) is not int or value['sequence'] < 0
            or type(value.get('observed_at_ms')) is not int or value['observed_at_ms'] <= 0
            or not isinstance(value.get('process_start_ticks'), str)
            or not value['process_start_ticks'].isdigit()
            or not isinstance(value.get('phase'), str) or len(value['phase']) > 80
            or not isinstance(value.get('hostname'), str) or len(value['hostname']) > 256
            or not isinstance(value.get('boot_id'), str) or len(value['boot_id']) > 64):
        return None
    return {key: value[key] for key in ('pid', 'process_start_ticks', 'hostname', 'boot_id',
                                       'sequence', 'observed_at_ms', 'state', 'phase')}


if __name__ == '__main__':
    try:
        result = read(*sys.argv[1:])
    except (OSError, ValueError, TypeError, KeyError):
        result = None
    print(json.dumps(result, sort_keys=True))
