"""Read-only ownership observation; never submit, cancel, rewrite or clean up."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import pwd
import stat
import subprocess
import sys


def read_regular(directory, name, maximum):
    descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=descriptor)
    finally:
        os.close(descriptor)
    with os.fdopen(fd, 'rb') as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_size > maximum:
            raise ValueError('bounded regular submission record required')
        raw = stream.read(maximum + 1)
        after = os.fstat(stream.fileno())
    stamp = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
    if len(raw) > maximum or stamp(before) != stamp(after):
        raise ValueError('submission record changed during observation')
    return raw


def verify_manifest(intent):
    root = Path(intent['remote_task_dir'])
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fd = os.open('task.json', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=descriptor)
    finally:
        os.close(descriptor)
    whole, dispatch = hashlib.sha256(), hashlib.sha256()
    identity = hashlib.sha256(str(root).encode() + b'\0')
    count, tail = 0, b''
    with os.fdopen(fd, 'rb') as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_size != intent['manifest_bytes']:
            raise ValueError('manifest size differs')
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            whole.update(block)
            count += len(block)
            data = tail + block
            dispatch.update(data[:-1])
            identity.update(data[:-1])
            tail = data[-1:]
        after = os.fstat(stream.fileno())
    if (tail != b'\n' or count != intent['manifest_bytes']
            or whole.hexdigest() != intent['manifest_sha256']
            or dispatch.hexdigest() != intent['dispatch_sha256']
            or identity.hexdigest() != intent['submission_identity']
            or (before.st_size, before.st_mtime_ns, before.st_ctime_ns)
            != (after.st_size, after.st_mtime_ns, after.st_ctime_ns)):
        raise ValueError('original submission content binding differs')
    if read_regular(root, 'submission.identity', 256) != (intent['submission_identity'] + '\n').encode():
        raise ValueError('submission identity differs')


def capture(argv):
    result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=30,
                            env={**os.environ, 'LC_ALL': 'C', 'TZ': 'UTC',
                                 'SLURM_TIME_FORMAT': 'standard'}, check=False)
    if len(result.stdout) > 1024 * 1024 or len(result.stderr) > 65536:
        raise ValueError('scheduler observation unavailable')
    return {'exit': result.returncode, 'stdout': result.stdout.decode('utf-8', errors='strict'),
            'stderr': result.stderr.decode('utf-8', errors='strict')}


def scheduler(argv):
    result = capture(argv)
    if result['exit'] != 0:
        raise ValueError('scheduler observation unavailable')
    return result['stdout']


def rows(text):
    result = []
    for line in text.splitlines():
        if not line.strip():
            continue
        parts = [part.strip() for part in line.split('|')]
        if len(parts) != 5 or not parts[0]:
            raise ValueError('ambiguous allocation ownership record')
        result.append(parts)
    return result


def observe(intent, *, query=scheduler, username=None):
    root = Path(intent['remote_task_dir'])
    result = {'schema': 'SLURM_SUBMISSION_OBSERVATION_V1',
              'submission_identity': intent['submission_identity'],
              'manifest_sha256': intent['manifest_sha256'],
              'remote_task_dir': str(root), 'state': 'UNRESOLVED',
              'job_id': None, 'queue': [], 'accounting': [],
              'resubmission_authorized': False}
    try:
        verify_manifest(intent)
    except FileNotFoundError:
        return result
    try:
        raw = read_regular(root, 'submission.job-id', 128).decode('ascii').strip()
    except FileNotFoundError:
        raw = ''
    # Federated IDs require cluster-specific observation, not a local-ID guess.
    if raw and not all('0' <= char <= '9' for char in raw):
        raise ValueError('unsupported or conflicting durable job ID')
    owner = username if username is not None else pwd.getpwuid(os.getuid()).pw_name
    queue_argv = ['squeue', '-h', '-u', owner, '-o', '%i|%.256u|%.100k|%.8192Z|%.256j']
    accounting_argv = ['sacct', '-n', '-P', '-X', '--duplicates',
                       '--format=JobIDRaw,User%256,Comment%100,WorkDir%8192,JobName%256']
    if raw:
        accounting_argv += ['-j', raw]
    else:
        since = datetime.datetime.fromtimestamp(
            max(0, int(intent['attempted_at_unix_ms']) / 1000 - 86400), datetime.timezone.utc)
        accounting_argv += ['-u', owner, '--starttime', since.strftime('%Y-%m-%dT%H:%M:%S')]
    queue, accounting = rows(query(queue_argv)), rows(query(accounting_argv))
    result.update(queue=queue, accounting=accounting)
    def owned(row):
        same_work = row[1] == owner and row[3] and os.path.realpath(row[3]) == os.path.realpath(root)
        same_comment = row[2] == 'autonomics-' + intent['submission_identity']
        # Some sites do not retain comments in accounting. The original,
        # pre-submission name carries its random nonce; never learn a name
        # from the scheduler and treat that newly learned value as expected.
        original_name = row[2] == '' and row[4] == intent['job_name']
        return same_work and (same_comment or original_name)
    candidates = {row[0] for row in queue + accounting if owned(row)}
    if raw:
        candidates &= {raw}
    if len(candidates) != 1:
        return result
    job = next(iter(candidates))
    if not job or not all('0' <= char <= '9' for char in job):
        raise ValueError('non-scalar recovered job ID')
    relevant = [row for row in queue + accounting if row[0] == job]
    if any(not owned(row) for row in relevant):
        raise ValueError('job ID conflicts with a different submission')
    if any(sum(row[0] == job for row in source) > 1 for source in (queue, accounting)):
        raise ValueError('multiple ownership records for one job ID')
    # Re-read the original records after the scheduler observation. No result
    # is accepted from counts or a mutable file learned after submission.
    verify_manifest(intent)
    result.update(state='IDENTITY_VERIFIED_JOB_FOUND', job_id=job)
    return result


def collect_stop_evidence(intent, *, command=capture, username=None):
    """Collect raw facts. No boolean in this result declares a job stopped.

    The native Store independently validates every step and the known retired
    controller response. This collector never cancels or closes a run.
    """
    def query(argv):
        result = command(argv)
        if result['exit'] != 0:
            raise ValueError('scheduler observation unavailable')
        return result['stdout']
    owned = observe(intent, query=query, username=username)
    if owned['state'] != 'IDENTITY_VERIFIED_JOB_FOUND':
        raise ValueError('remote allocation ownership remains unresolved')
    job = owned['job_id']
    owner = username if username is not None else pwd.getpwuid(os.getuid()).pw_name
    queue_argv = ['squeue', '-h', '-u', owner, '-o', '%i|%T']
    before = command(queue_argv)
    accounting = command(['sacct', '-n', '-P', '--duplicates', '-j', job,
        '--format=JobIDRaw,State%40,ExitCode,End,User%256,Comment%100,WorkDir%8192,JobName%256'])
    unallocated = None
    lines = [line.strip().split('|') for line in accounting['stdout'].splitlines() if line.strip()]
    if accounting['exit'] == 0 and len(lines) == 1 and len(lines[0]) == 8 and lines[0][0] == job:
        # Facts only: the native consumer must require actual zero allocation,
        # no start/restart and original terminal identity, not merely no steps.
        unallocated = command(['sacct', '-n', '-P', '--duplicates', '-j', job,
            '--format=JobIDRaw,State%40,ExitCode,Start,End,AllocCPUS,AllocNodes,AllocTRES%256,NodeList%1024,Restarts,User%256,Comment%100,WorkDir%8192,JobName%256'])
    control = command(['scontrol', 'show', 'job', '-o', job])
    after = command(queue_argv)
    verify_manifest(intent)
    result = {'schema': 'SLURM_OWNED_STOP_PROOF_V1', 'job_id': job,
            'ownership': owned, 'queue': before, 'accounting': accounting,
            'control': control, 'queue_after': after}
    if unallocated is not None:
        result['unallocated_accounting'] = unallocated
    return result


def main():
    try:
        if len(sys.argv) not in (2, 3) or len(sys.argv[1].encode()) > 16384:
            raise ValueError('bounded recovery request required')
        if len(sys.argv) == 3 and sys.argv[2] != 'collect-stop':
            raise ValueError('unsupported observation mode')
        operation = collect_stop_evidence if len(sys.argv) == 3 else observe
        print(json.dumps(operation(json.loads(sys.argv[1])), sort_keys=True))
    except Exception:
        print('Slurm submission recovery remains unknown; evidence retained', file=sys.stderr)
        raise SystemExit(2)


if __name__ == '__main__':
    main()
