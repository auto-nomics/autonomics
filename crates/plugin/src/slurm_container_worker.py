"""Ordinary same-UID Apptainer worker for the existing container node contract.

Invoked only inside the recorded Slurm allocation. No image build/pull, daemon,
privilege emulation, scientific authorization, or remote scratch deletion.
"""
import ctypes
import errno
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import resource
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time


class WorkerAdmissionError(ValueError):
    """A fixed public error code; never contains a path, payload or credential."""
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


class NamespacePreparationError(WorkerAdmissionError):
    def __init__(self, failure):
        super().__init__('SHARED_MEMORY_HELPER_FAILED', 'private namespace helper failed')
        self.failure = failure


class WorkerDiagnostics:
    def __init__(self):
        self.phase = 'allocation_admission'
        self.tool_exit_code = None
        self.software = None

    def failure(self, error):
        if isinstance(error, NamespacePreparationError):
            return error.failure
        code = error.code if isinstance(error, WorkerAdmissionError) else 'WORKER_INTERNAL_ERROR'
        if isinstance(error, OSError):
            code = {errno.ENOSPC: 'FILESYSTEM_NO_SPACE', errno.EDQUOT: 'FILESYSTEM_QUOTA',
                    errno.EACCES: 'FILESYSTEM_ACCESS_DENIED', errno.EPERM: 'OPERATION_NOT_PERMITTED',
                    errno.ENOENT: 'REQUIRED_FILE_NOT_FOUND', errno.EEXIST: 'DESTINATION_EXISTS',
                    errno.EROFS: 'FILESYSTEM_READ_ONLY', errno.EINVAL: 'INVALID_SYSTEM_OPERATION',
                    errno.ENOSYS: 'SYSTEM_OPERATION_UNAVAILABLE',
                    errno.EAGAIN: 'RESOURCE_TEMPORARILY_UNAVAILABLE'}.get(error.errno, 'WORKER_OS_ERROR')
        elif isinstance(error, (KeyError, TypeError, json.JSONDecodeError)):
            code = 'INVALID_WORKER_REQUEST'
        elif isinstance(error, subprocess.TimeoutExpired):
            code = 'WORKER_SUBPROCESS_TIMEOUT'
        return {'schema': 'APPTAINER_WORKER_FAILURE_V1', 'phase': self.phase,
                'code': code, 'tool_exit_code': self.tool_exit_code,
                'errno': error.errno if isinstance(error, OSError) and type(error.errno) is int
                         and 0 <= error.errno <= 4095 else None}


def phase(diagnostics, value):
    if diagnostics is not None:
        diagnostics.phase = value



class WorkerHeartbeat:
    """Advisory same-process heartbeat, bound to the exact transport manifest."""
    def __init__(self, root, *, start=True):
        self.root = root
        manifest_path = regular(root, 'task.json')
        if manifest_path.stat().st_size > 16 * 1024 * 1024:
            raise WorkerAdmissionError('MANIFEST_TOO_LARGE', 'bounded manifest required')
        manifest = manifest_path.read_bytes()
        task = json.loads(manifest)['spec']['id']
        job = os.environ.get('SLURM_JOB_ID', '')
        if not job.isdigit():
            raise WorkerAdmissionError('SLURM_IDENTITY_MISSING', 'Slurm identity required')
        start_ticks = Path('/proc/self/stat').read_text().rsplit(')', 1)[1].split()[19]
        self.identity = {'schema': 'AUTONOMICS_WORKER_HEARTBEAT_V1', 'task_id': task,
            'job_id': job, 'manifest_sha256': hashlib.sha256(manifest).hexdigest(),
            'pid': os.getpid(), 'process_start_ticks': start_ticks,
            'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
            'hostname': Path('/proc/sys/kernel/hostname').read_text().strip()}
        self.current_phase = 'preparing_worker'
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._run, name='worker-observation', daemon=True)
        self.started = False
        self.sequence = 0
        if start:
            self.start()
        else:
            self._sample('active')

    def start(self):
        if not self.started:
            try:
                self.thread.start()
                self.started = True
            except (OSError, RuntimeError):
                pass  # Telemetry is advisory, including thread admission.

    def phase(self, value):
        self.current_phase = value
        if not self.started:
            self._sample('active')

    def _sample(self, state):
        try:
            self._write(self.sequence, state)
        except (OSError, ValueError):
            pass
        self.sequence += 1

    def _write(self, sequence, state):
        value = dict(self.identity, phase=self.current_phase, sequence=sequence,
                     state=state, observed_at_ms=int(time.time() * 1000))
        descriptor, path = tempfile.mkstemp(prefix='.worker-heartbeat-', dir=self.root)
        try:
            with os.fdopen(descriptor, 'w') as stream:
                json.dump(value, stream, sort_keys=True)
            os.replace(path, self.root / '.autonomics-worker-heartbeat.json')
        finally:
            if os.path.exists(path):
                os.unlink(path)

    def _run(self):
        while True:
            stopping = self.stop.is_set()
            self._sample('exiting' if stopping else 'active')
            if stopping:
                return
            self.stop.wait(5)

    def close(self):
        self.stop.set()
        if self.started:
            self.thread.join()
        else:
            self._sample('exiting')


def sha(path):
    result = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            result.update(chunk)
    return result.hexdigest()


def relative(value):
    path = PurePosixPath(value)
    if (not value or path.is_absolute() or str(path) != value
            or any(part in ('', '.', '..') for part in path.parts)
            or any(ord(char) < 32 for char in value) or '\\' in value):
        raise WorkerAdmissionError('NONCANONICAL_PATH', 'noncanonical relative path')
    return path


def regular(root, value):
    path = root
    for part in relative(value).parts:
        path = path / part
        if path.is_symlink():
            raise WorkerAdmissionError('INPUT_SYMLINK_REFUSED', 'symlink refused in admitted input')
    if not stat.S_ISREG(path.stat().st_mode):
        raise WorkerAdmissionError('INPUT_NOT_REGULAR', 'regular file required')
    return path


def file_identity(path, record):
    expected = record['sha256']
    if expected.startswith('sha256:'):
        expected = expected[len('sha256:'):]
    if path.stat().st_size != record['size'] or sha(path) != expected:
        raise WorkerAdmissionError('INPUT_IDENTITY_MISMATCH', 'staged file identity differs')


def write_new(path, value):
    with path.open('x') as stream:
        json.dump(value, stream, sort_keys=True)
        stream.flush()
        os.fsync(stream.fileno())


def verify_resource_probe(root, request, storage, *, launch=subprocess.run):
    """Run the frozen native allocation probe before any method or credentials.

    Ordinary jobs have no such request. The probe records resource facts only;
    its successful exit never grants scientific or method authorization.
    """
    probe = request.get('resource_probe')
    if probe is None:
        return None
    if set(probe) != {'program', 'budget'}:
        raise ValueError('closed native resource probe request required')
    program, budget = probe['program'], probe['budget']
    if set(program) != {'path', 'sha256', 'bytes', 'engine_revision'}:
        raise ValueError('complete native resource program identity required')
    integers = ('max_concurrency', 'cpus', 'memory_bytes', 'timeout_ms',
                'memory_available_reserve_bytes', 'peak_disk_bytes')
    if (set(budget) != set(integers) | {'disk_reserve_fraction'}
            or any(type(budget[k]) is not int or not 0 <= budget[k] <= (1 << 64) - 1
                   for k in integers)
            or budget['max_concurrency'] != 1
            or any(budget[k] == 0 for k in ('cpus', 'memory_bytes', 'timeout_ms', 'peak_disk_bytes'))
            or budget['memory_bytes'] + budget['memory_available_reserve_bytes'] > (1 << 64) - 1
            or type(budget['disk_reserve_fraction']) not in (int, float)
            or not 0 <= budget['disk_reserve_fraction'] < 1):
        raise ValueError('complete exact remote resource budget required')
    def hex_value(value, length):
        return isinstance(value, str) and len(value) == length and all(c in '0123456789abcdef' for c in value)
    path = Path(program['path'])
    if (not path.is_absolute() or path.is_symlink() or path.resolve(strict=True) != path
            or not path.is_file() or path.stat().st_mode & 0o022
            or not os.access(path, os.X_OK)
            or type(program['bytes']) is not int or not 0 < program['bytes'] <= 2 * 1024**3
            or not hex_value(program['sha256'], 64) or not hex_value(program['engine_revision'], 40)):
        raise ValueError('canonical locked native resource probe required')
    file_identity(path, {'size': program['bytes'], 'sha256': program['sha256']})
    job = os.environ.get('SLURM_JOB_ID', '')
    if not job or not all('0' <= char <= '9' for char in job):
        raise ValueError('actual Slurm allocation identity required')
    budget_path = root / 'resource-budget.json'
    write_new(budget_path, budget)
    argv = [str(path), 'inspect-remote-resources', str(budget_path), str(storage)]
    write_new(root / 'resource-probe-intent.json', {'argv': argv, 'program': program})
    def bounded_logs():
        resource.setrlimit(resource.RLIMIT_FSIZE, (65536, 65536))
    stdout, stderr = root / 'resource-probe.stdout', root / 'resource-probe.stderr'
    with stdout.open('xb') as out, stderr.open('xb') as err:
        completed = launch(argv, cwd=root, env={'PATH': '/usr/bin:/bin', 'LANG': 'C',
                           'SLURM_JOB_ID': job}, stdin=subprocess.DEVNULL,
                           stdout=out, stderr=err, check=False, timeout=60,
                           preexec_fn=bounded_logs)
    write_new(root / 'resource-probe-wait.json', {'actual_wait': True, 'exit_code': completed.returncode})
    file_identity(path, {'size': program['bytes'], 'sha256': program['sha256']})
    if completed.returncode != 0 or stdout.stat().st_size > 65536 or stderr.stat().st_size > 65536:
        raise ValueError('native remote resource probe did not complete successfully')
    def closed(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError('duplicate resource proof field')
            value[key] = item
        return value
    record = json.loads(stdout.read_bytes(), object_pairs_hook=closed)
    if (record.get('schema') != 'FORMAL_REMOTE_RESOURCE_PROBE_V1'
            or record.get('job_id') != job or record.get('engine_revision') != program['engine_revision']
            or record.get('budget') != budget or record.get('storage') != str(storage)
            or record.get('kernel_limits_verified') is not True
            or record.get('readiness', {}).get('ready') is not True
            or record.get('formal_authorized') is not False
            or record.get('scientific_gate_granted') is not False):
        raise ValueError('native remote resource proof differs from its request')
    return record


def verify_panel(panel):
    root = Path(panel['root'])
    if not root.is_absolute() or root != root.resolve() or not root.is_dir():
        raise WorkerAdmissionError('PANEL_CACHE_NOT_CANONICAL', 'canonical panel cache required')
    manifest = regular(root, 'manifest.json')
    if sha(manifest) != panel['manifest_sha256']:
        raise WorkerAdmissionError('PANEL_MANIFEST_MISMATCH', 'panel manifest identity differs')
    value = json.loads(manifest.read_bytes())
    if value['digest'] != panel['digest'] or value.get('id', value.get('repo')) != panel['id']:
        raise WorkerAdmissionError('PANEL_BINDING_MISMATCH', 'panel binding differs')
    for item in value['files']:
        file_identity(regular(root, item['path']), item)


def safe_environment(values):
    output = {}
    for name, value in values:
        if (not name or name[0].isdigit() or any(ord(c) > 127 for c in name)
                or not all(c.isalnum() or c == '_' for c in name)
                or name.startswith(('APPTAINER', 'SINGULARITY')) or '\0' in value):
            raise WorkerAdmissionError('INVALID_PAYLOAD_ENVIRONMENT', 'invalid payload environment')
        if name in ('HOME', 'TMPDIR', 'TMP', 'TEMP'):
            raise WorkerAdmissionError('MANAGED_ENVIRONMENT_OVERRIDE', 'payload cannot replace managed home/tmp')
        if name in output:
            raise WorkerAdmissionError('DUPLICATE_PAYLOAD_ENVIRONMENT', 'duplicate payload environment')
        output[name] = value
    return output


def inherit_egress_proxies(values, environment):
    """Preserve the scheduler's site egress route without importing its shell.

    A declared spelling, even an empty value, overrides both inherited case
    variants: requests and other clients may prefer the lowercase spelling.
    Values travel only in the child environment, never arguments or receipts.
    """
    inherited = []
    for lower in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy'):
        names = (lower, lower.upper())
        if any(name in values for name in names):
            continue
        for name in names:
            value = environment.get(name)
            if value:
                values[name] = value
                inherited.append(name)
    return inherited


def contains_secret(path, secrets):
    needles = [value.encode() for value in secrets.values()]
    if not needles:
        return False
    overlap = max(map(len, needles)) - 1
    tail = b''
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            chunk = tail + chunk
            if any(value in chunk for value in needles):
                return True
            tail = chunk[-overlap:] if overlap else b''
    return False


def private_shared_memory(path, size):
    """Mount a capped tmpfs in this worker's same-UID private namespace.

    A single identity is mapped to itself; UID zero is never mapped. Namespace
    capabilities exist only while mounting this private filesystem, then are
    cleared before Apptainer starts in its own unprivileged user namespace.
    The mount dies with this Slurm worker and its foreground descendants.
    """
    page = os.sysconf('SC_PAGE_SIZE')
    if type(size) is not int or size <= 0 or size % page or size > (1 << 63) - 1:
        raise WorkerAdmissionError('INVALID_SHARED_MEMORY_CAPACITY', 'shared-memory capacity must be a positive exact page multiple')
    uid, gid = os.getuid(), os.getgid()
    if not uid:
        raise WorkerAdmissionError('SHARED_MEMORY_ROOT_REFUSED', 'shared-memory namespace requires a nonroot identity')
    path.mkdir(mode=0o700)
    libc = ctypes.CDLL(None, use_errno=True)
    def call(name, *args):
        if getattr(libc, name)(*args) != 0:
            raise OSError(ctypes.get_errno(), 'private shared-memory ' + name + ' failed')
    # CLONE_NEWUSER | CLONE_NEWNS. Neither subordinate IDs nor root mapping.
    call('unshare', 0x10000000 | 0x00020000)
    Path('/proc/self/setgroups').write_text('deny')
    Path('/proc/self/uid_map').write_text('%s %s 1\n' % (uid, uid))
    Path('/proc/self/gid_map').write_text('%s %s 1\n' % (gid, gid))
    # MS_REC | MS_PRIVATE prevents changes propagating to the host namespace.
    call('mount', None, b'/', None, ctypes.c_ulong((1 << 14) | (1 << 18)), None)
    options = 'size=%s,mode=0700,uid=%s,gid=%s' % (size, uid, gid)
    # MS_NOSUID | MS_NODEV | MS_NOEXEC; shared memory remains ordinary user data.
    call('mount', b'tmpfs', os.fsencode(str(path)), b'tmpfs', ctypes.c_ulong(2 | 4 | 8), options.encode())
    measured = os.statvfs(str(path))
    if measured.f_blocks * measured.f_frsize != size:
        raise WorkerAdmissionError('SHARED_MEMORY_CAPACITY_MISMATCH', 'mounted shared-memory capacity differs')
    class Header(ctypes.Structure):
        _fields_ = [('version', ctypes.c_uint32), ('pid', ctypes.c_int)]
    class Data(ctypes.Structure):
        _fields_ = [('effective', ctypes.c_uint32), ('permitted', ctypes.c_uint32),
                    ('inheritable', ctypes.c_uint32)]
    call('capset', ctypes.byref(Header(0x20080522, 0)), ctypes.byref((Data * 2)()))
    call('prctl', 38, 1, 0, 0, 0)  # PR_SET_NO_NEW_PRIVS
    if (os.getuid(), os.getgid()) != (uid, gid):
        raise WorkerAdmissionError('SHARED_MEMORY_IDENTITY_MISMATCH', 'shared-memory namespace changed the execution identity')


def private_shm_exec(arguments):
    """Exec Apptainer in a single-threaded, foreground private namespace.

    Mounts disappear with this foreground invocation. The coordinator never
    enters the namespace, so its measured workspace cleanup cannot hit a live
    tmpfs mount, and no privileged cleanup daemon is needed.
    """
    diagnostics = WorkerDiagnostics()
    diagnostics.phase = 'shared_memory_admission'
    runtime = Path(arguments[0])
    try:
        job = os.environ.get('SLURM_JOB_ID', '')
        if (not job.isdigit() or not any(('job_' + job + '/') in line or ('job-' + job + '/') in line
                for line in Path('/proc/self/cgroup').read_text().splitlines())):
            raise WorkerAdmissionError('SLURM_ALLOCATION_MISMATCH', 'namespace helper is outside its Slurm allocation')
        if (len(arguments) < 5 or arguments[2] != '--' or not arguments[1].isdigit()
                or not runtime.is_absolute() or runtime.resolve(strict=True) != runtime
                or runtime.stat().st_uid != os.getuid() or stat.S_IMODE(runtime.stat().st_mode) != 0o700):
            raise WorkerAdmissionError('INVALID_SHARED_MEMORY_CAPACITY', 'invalid private namespace invocation')
        if len(list(Path('/proc/self/task').iterdir())) != 1:
            raise WorkerAdmissionError('NAMESPACE_HELPER_NOT_SINGLE_THREADED', 'namespace helper must be single threaded')
        size = int(arguments[1])
        private_shared_memory(runtime / 'shm', size)
        write_new(runtime / 'shm-prepared.json', {'schema': 'APPTAINER_SHM_PREPARED_V1',
                  'bytes': size, 'uid': os.getuid(), 'job_id': job})
        diagnostics.phase = 'container_exec'
        os.execvpe(arguments[3], arguments[3:], os.environ)
    except Exception as error:
        write_new(runtime / 'shm-failure.json', diagnostics.failure(error))
        return 125


def copy_measured(source, target, expected):
    """Cross-filesystem export/staging without replacing an existing object."""
    before = source.stat()
    with source.open('rb') as src, target.open('xb') as dst:
        shutil.copyfileobj(src, dst, 1024 * 1024)
        dst.flush()
        os.fsync(dst.fileno())
    stamp = lambda info: (info.st_dev, info.st_ino, info.st_size,
                          info.st_mtime_ns, info.st_ctime_ns)
    if stamp(before) != stamp(source.stat()):
        raise WorkerAdmissionError('COPY_SOURCE_CHANGED', 'source changed during cross-filesystem copy')
    if target.stat().st_size != before.st_size or sha(target) != expected:
        raise WorkerAdmissionError('COPY_IDENTITY_MISMATCH', 'cross-filesystem copy identity differs')


def require_atomic_publication(root):
    """Test the real directory primitive, never downgrade to clobbering rename."""
    libc = ctypes.CDLL(None, use_errno=True)
    rename = getattr(libc, 'renameat2', None)
    if rename is None:
        raise WorkerAdmissionError('ATOMIC_PUBLICATION_UNAVAILABLE', 'node scratch lacks renameat2')
    rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int,
                       ctypes.c_char_p, ctypes.c_uint]
    rename.restype = ctypes.c_int
    with tempfile.TemporaryDirectory(prefix='atomic-probe-', dir=root) as directory:
        source, target = Path(directory) / 'source', Path(directory) / 'target'
        source.mkdir()
        (source / 'marker').write_bytes(b'original')
        if rename(-100, os.fsencode(source), -100, os.fsencode(target), 1) != 0:
            raise WorkerAdmissionError('ATOMIC_PUBLICATION_UNSUPPORTED', 'node scratch lacks atomic no-replace directory publication')
        source.mkdir()
        (source / 'marker').write_bytes(b'replacement')
        ctypes.set_errno(0)
        import errno
        if (rename(-100, os.fsencode(source), -100, os.fsencode(target), 1) != -1
                or ctypes.get_errno() != errno.EEXIST
                or (target / 'marker').read_bytes() != b'original'
                or (source / 'marker').read_bytes() != b'replacement'):
            raise WorkerAdmissionError('ATOMIC_REPLACEMENT_PROTECTION_FAILED', 'node scratch replacement protection failed')


@contextmanager
def node_workspace(root, request, diagnostics=None):
    """GPFS is transport storage; method work needs real no-replace publication.

    This is node-local scratch inside the same Slurm job, not a cloned software
    environment. The original shared inputs remain intact. A failed/unknown
    execution preserves its scratch rather than asserting stopped-consumer cleanup.
    """
    phase(diagnostics, 'workspace_capacity')
    total = 0
    for item in request['files']:
        if type(item['size']) is not int or item['size'] < 0:
            raise WorkerAdmissionError('INVALID_INPUT_SIZE', 'exact staged input size required')
        total += item['size']
    # Slurm's allocation scratch takes precedence over a coordinator's shared
    # TMPDIR. Capacity and atomic-publication admission still run on this exact
    # directory; a failure never falls back to another filesystem.
    parent = Path(os.environ.get('SLURM_TMPDIR') or tempfile.gettempdir()).resolve(strict=True)
    capacity = os.statvfs(parent)
    required = 4 * 1024**3 + 2 * total
    if capacity.f_bavail * capacity.f_frsize < required:
        raise WorkerAdmissionError('WORKSPACE_CAPACITY_INSUFFICIENT', 'insufficient node-local workspace capacity')
    work = Path(tempfile.mkdtemp(prefix='autonomics-container-', dir=parent))
    launched = False
    try:
        phase(diagnostics, 'workspace_atomic_publication')
        require_atomic_publication(work)
        phase(diagnostics, 'input_staging')
        payload = work / 'payload'
        payload.mkdir(mode=0o700)
        for name in request['directories']:
            payload.joinpath(*relative(name).parts).mkdir(mode=0o700, parents=True, exist_ok=True)
        for item in request['files']:
            source = regular(root / 'payload', item['path'])
            file_identity(source, item)
            target = payload.joinpath(*relative(item['path']).parts)
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            copy_measured(source, target, sha(source))
            target.chmod(item['mode'] & 0o777)
        write_new(root / 'node-workspace.json', {
            'schema': 'SLURM_NODE_LOCAL_CONTAINER_WORKSPACE_V1',
            'path': str(work), 'job_id': os.environ.get('SLURM_JOB_ID'),
            'input_bytes': total, 'required_free_bytes': required,
            'observed_available_bytes': capacity.f_bavail * capacity.f_frsize,
            'capacity_is_reservation': False, 'atomic_noreplace_verified': True,
            'scientific_authorization_granted': False})
        launched = True
        yield work
    except BaseException:
        if launched:
            # A thrown launch/wait is not proof that all consumers stopped.
            # Keep the named node-local scratch for explicit job recovery.
            raise
        shutil.rmtree(work)
        raise
    else:
        phase(diagnostics, 'workspace_cleanup')
        shutil.rmtree(work)


def execute(root, request, *, launch=subprocess.run, work_root=None, heartbeat=None, diagnostics=None):
    if heartbeat is not None:
        heartbeat.phase('preparing_container')
    root = root.resolve()
    work_root = root if work_root is None else work_root.resolve(strict=True)
    phase(diagnostics, 'resource_probe_admission')
    resource_admission = verify_resource_probe(root, request, work_root, launch=launch)
    payload = work_root / 'payload'
    payload.mkdir(mode=0o700, exist_ok=True)
    for name in request['directories']:
        payload.joinpath(*relative(name).parts).mkdir(mode=0o700, parents=True, exist_ok=True)
    originals = {}
    phase(diagnostics, 'input_identity')
    for item in request['files']:
        source = regular(payload, item['path'])
        file_identity(source, item)
        source.chmod(item['mode'] & 0o777)
        originals[item['path']] = item['sha256']
    phase(diagnostics, 'image_identity')
    sif = Path(request['sif']['path'])
    if not sif.is_absolute() or sif.is_symlink() or not sif.is_file() or sha(sif) != request['sif']['sha256']:
        raise WorkerAdmissionError('SIF_IDENTITY_MISMATCH', 'SIF identity differs')
    if diagnostics is not None and diagnostics.software is not None:
        # The preceding byte comparison is the actual measurement. Do not
        # manufacture an observed image identity from the request alone.
        diagnostics.software['sif'] = {'path': str(sif), 'sha256': request['sif']['sha256'],
                                       'bytes': sif.stat().st_size}
    phase(diagnostics, 'panel_identity')
    for panel in request['panels']:
        verify_panel(panel)
    phase(diagnostics, 'execution_identity')
    if request.get('user') is not None:
        parts = request['user'].split(':')
        if int(parts[0]) != os.getuid() or (len(parts) == 2 and int(parts[1]) != os.getgid()):
            raise WorkerAdmissionError('EXECUTION_IDENTITY_MISMATCH', 'requested user differs from allocated site identity')
    if os.getuid() == 0:
        raise WorkerAdmissionError('ROOT_WORKER_REFUSED', 'ordinary site worker must not run as root')
    phase(diagnostics, 'runtime_preparation')
    runtime = work_root / 'runtime'
    runtime.mkdir(mode=0o700)
    for name in ('home', 'tmp', 'var-tmp', 'cache', 'session'):
        (runtime / name).mkdir(mode=0o700)
    env = {'PATH': '/usr/bin:/bin', 'LANG': 'C', 'HOME': str(runtime / 'home'),
           'TMPDIR': str(runtime / 'tmp'), 'APPTAINER_TMPDIR': str(runtime / 'session'),
           'APPTAINER_CACHEDIR': str(runtime / 'cache')}
    phase(diagnostics, 'environment_admission')
    values = safe_environment(request['env'])
    inherited_proxies = []
    if request['network'] == 'egress':
        inherited_proxies = inherit_egress_proxies(values, os.environ)
    secrets = {}
    secret_path = root / 'private-environment.json'
    if request.get('private_environment'):
        if secret_path.is_symlink():
            raise WorkerAdmissionError('PRIVATE_ENVIRONMENT_SYMLINK_REFUSED', 'private environment symlink refused')
        info = secret_path.stat()
        if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
            raise WorkerAdmissionError('PRIVATE_ENVIRONMENT_OWNERSHIP_MISMATCH', 'private environment ownership differs')
        secrets = json.loads(secret_path.read_bytes())
        secret_path.unlink()
        if (set(secrets) != {'OPENGWAS_JWT'} or not secrets['OPENGWAS_JWT'].strip()
                or any(ord(c) < 32 for c in secrets['OPENGWAS_JWT'])):
            raise WorkerAdmissionError('INVALID_PRIVATE_ENVIRONMENT', 'invalid private environment')
        if set(values).intersection(secrets):
            raise WorkerAdmissionError('DUPLICATE_PRIVATE_ENVIRONMENT', 'duplicate private environment')
        values.update(secrets)
    values.update(HOME='/home/autonomics', TMPDIR='/tmp')
    env.update(('APPTAINERENV_' + name, value) for name, value in values.items() if name != 'HOME')
    phase(diagnostics, 'gpu_admission')
    if request.get('gpus'):
        visible = os.environ.get('CUDA_VISIBLE_DEVICES', '')
        allocated = os.environ.get('SLURM_STEP_GPUS') or os.environ.get('SLURM_JOB_GPUS', '')
        count = request.get('gpu_count')
        if (not visible or any(part.strip() in ('', '-1', 'NoDevFiles') for part in visible.split(','))
                or not allocated or type(count) is not int or count <= 0
                or len(visible.split(',')) != count):
            raise WorkerAdmissionError('GPU_ALLOCATION_MISMATCH', 'requested GPU allocation visibility is absent or differs')
        if 'CUDA_VISIBLE_DEVICES' in values:
            raise WorkerAdmissionError('GPU_VISIBILITY_OVERRIDE', 'payload cannot replace scheduler GPU visibility')
        env['APPTAINERENV_CUDA_VISIBLE_DEVICES'] = visible
    program = (diagnostics.software['container_runtime']['path']
               if diagnostics is not None and diagnostics.software is not None else request['program'])
    command = [program, 'exec', '--cleanenv', '--no-eval', '--contain', '--ipc',
               '--home', str(runtime / 'home') + ':/home/autonomics',
               '--no-mount', 'cwd,hostfs,bind-paths',
               '--bind', str(payload) + ':' + request['workdir'] + ':rw',
               '--bind', str(runtime / 'tmp') + ':/tmp:rw',
               '--bind', str(runtime / 'var-tmp') + ':/var/tmp:rw',
               '--workdir', str(runtime / 'session'), '--pwd', request['workdir']]
    for panel in request['panels']:
        command += ['--bind', panel['root'] + ':' + panel['mount_path'] + ':ro']
    for item in request['files']:
        if item.get('readonly'):
            command += ['--bind', str(payload / item['path']) + ':' + request['workdir'] + '/' + item['path'] + ':ro']
    if request['network'] == 'isolated':
        command += ['--net', '--network', 'none']
    elif request['network'] == 'egress':
        # Some contained SIF runs expose an empty generated resolv.conf even
        # when the allocation's resolver works. Use the execution node's
        # configured resolver, without changing nameservers or network scope.
        command += ['--bind', '/etc/resolv.conf:/etc/resolv.conf:ro']
    else:
        raise WorkerAdmissionError('UNSUPPORTED_NETWORK_PROFILE', 'unsupported network profile')
    if not request['read_only_rootfs']:
        command += ['--writable-tmpfs']
    if request.get('gpus'):
        command += ['--nv']
    if request.get('shm_size') is not None:
        phase(diagnostics, 'shared_memory_admission')
        command += ['--userns', '--bind', str(runtime / 'shm') + ':/dev/shm:rw']
    elif request.get('shm_size_bytes') is not None:
        raise WorkerAdmissionError('SHARED_MEMORY_REQUEST_MISSING', 'shared-memory byte capacity lacks its declared request')
    command += [str(sif)] + request['command']
    if request.get('shm_size') is not None:
        env['SLURM_JOB_ID'] = os.environ.get('SLURM_JOB_ID', '')
        command = [sys.executable, '-B', str(Path(__file__).resolve()), '--private-shm-exec',
                   str(runtime), str(request.get('shm_size_bytes')), '--'] + command
    def limits():
        if request.get('pids_limit') is not None:
            # This is a conservative per-UID bound, stricter than a per-job cap.
            count = request['pids_limit']
            old = resource.getrlimit(resource.RLIMIT_NPROC)
            hard = count if old[1] == resource.RLIM_INFINITY else min(count, old[1])
            resource.setrlimit(resource.RLIMIT_NPROC, (hard, hard))
    stdout_path, stderr_path = root / 'payload.stdout', root / 'payload.stderr'
    if heartbeat is not None:
        heartbeat.phase('executing')
        # Namespace setup runs in a separate single-threaded exec helper; this
        # coordinator and its telemetry thread never acquire private mounts.
        heartbeat.start()
    phase(diagnostics, 'container_launch_and_wait')
    with stdout_path.open('xb') as stdout, stderr_path.open('xb') as stderr:
        result = launch(command, cwd=root, env=env, stdin=subprocess.DEVNULL,
                        stdout=subprocess.DEVNULL if secrets else stdout,
                        stderr=subprocess.DEVNULL if secrets else stderr,
                        preexec_fn=limits, check=False)
    if request.get('shm_size') is not None:
        failure_path = runtime / 'shm-failure.json'
        if failure_path.exists():
            failure = json.loads(regular(runtime, 'shm-failure.json').read_bytes())
            # The helper owns this private control path; the container only
            # receives payload/home/tmp binds and cannot write its receipts.
            if (failure.get('schema') != 'APPTAINER_WORKER_FAILURE_V1'
                    or failure.get('phase') not in ('shared_memory_admission', 'container_exec')):
                raise WorkerAdmissionError('SHARED_MEMORY_HELPER_RECEIPT_INVALID', 'invalid namespace helper failure receipt')
            raise NamespacePreparationError(failure)
        prepared = json.loads(regular(runtime, 'shm-prepared.json').read_bytes())
        if prepared != {'schema': 'APPTAINER_SHM_PREPARED_V1', 'bytes': request.get('shm_size_bytes'),
                        'uid': os.getuid(), 'job_id': env['SLURM_JOB_ID']}:
            raise WorkerAdmissionError('SHARED_MEMORY_HELPER_RECEIPT_INVALID', 'namespace helper preparation differs')
    if diagnostics is not None:
        diagnostics.tool_exit_code = result.returncode if result.returncode >= 0 else 128 - result.returncode
    phase(diagnostics, 'output_admission')
    export = root / 'export'
    export.mkdir(mode=0o700)
    if heartbeat is not None:
        heartbeat.phase('collecting_outputs')
    outputs = []
    # A job is finished before export. Every returned byte is measured, and all
    # output links/special files are refused before the coordinator can restore.
    for directory, dirs, files in os.walk(payload, followlinks=False):
        for name in dirs:
            if (Path(directory) / name).is_symlink():
                raise WorkerAdmissionError('OUTPUT_DIRECTORY_SYMLINK_REFUSED', 'output directory symlink refused')
        for name in sorted(files):
            source = Path(directory) / name
            if source.is_symlink() or not source.is_file():
                raise WorkerAdmissionError('OUTPUT_NOT_REGULAR', 'output must be a regular nonsymlink file')
            rel = source.relative_to(payload).as_posix()
            relative(rel)
            digest = sha(source)
            if originals.get(rel) == digest:
                continue
            if contains_secret(source, secrets):
                raise WorkerAdmissionError('OUTPUT_CREDENTIAL_REFUSED', 'private credential in payload output; export refused')
            index = len(outputs)
            target = export / str(index)
            # Node-local method output is exported to the shared transport
            # directory only after a measured, exclusive, complete byte copy.
            copy_measured(source, target, digest)
            outputs.append({'path': rel, 'index': index, 'size': source.stat().st_size,
                            'sha256': digest, 'mode': stat.S_IMODE(source.stat().st_mode)})
    def capture(path):
        with path.open('rb') as stream:
            raw = stream.read(65537)
        return raw[:65536].decode(errors='replace') + ('\n[truncated]' if len(raw) > 65536 else '')
    # Negative signal return codes are preserved as conventional 128+signal.
    code = result.returncode if result.returncode >= 0 else 128 - result.returncode
    returned = {'schema': 'APPTAINER_CONTAINER_RESULT_V1', 'exit_code': code,
            'stdout': '' if secrets else capture(stdout_path),
            'stderr': 'private environment: captured output suppressed' if secrets else capture(stderr_path),
            'files': outputs, 'uid': os.getuid(), 'cgroup': Path('/proc/self/cgroup').read_text(),
            'pids_policy': 'per_uid_rlimit' if request.get('pids_limit') else None,
            'shm_size_bytes': request.get('shm_size_bytes'),
            'network': request['network'], 'inherited_proxy_names': inherited_proxies}
    if resource_admission is not None:
        returned['resource_admission'] = resource_admission
    return returned


def measured_software_file(path):
    """Measure stable regular software bytes; never inspect an environment."""
    running = str(path) == '/proc/self/exe'
    path = Path(path) if running else Path(path).resolve(strict=True)
    # Hold the running inode itself. Resolving /proc/self/exe and then opening
    # its target name could measure a replacement installed after exec.
    descriptor = os.open(str(path), os.O_RDONLY | os.O_NONBLOCK | (0 if running else os.O_NOFOLLOW))
    try:
        observed_path = os.readlink(str(path)) if running else str(path)
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            raise WorkerAdmissionError('SOFTWARE_IDENTITY_INVALID', 'regular software required')
        digest = hashlib.sha256()
        while True:
            block = os.read(descriptor, 1024 * 1024)
            if not block:
                break
            digest.update(block)
        def identity(value):
            return (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns)
        if identity(before) != identity(os.fstat(descriptor)) or identity(before) != identity(path.stat()):
            raise WorkerAdmissionError('SOFTWARE_IDENTITY_CHANGED', 'software changed during measurement')
        return {'path': observed_path, 'sha256': digest.hexdigest(), 'bytes': before.st_size}
    finally:
        os.close(descriptor)


def software_evidence(root, request):
    manifest_path = regular(root, 'task.json')
    if manifest_path.stat().st_size > 16 * 1024 * 1024:
        raise WorkerAdmissionError('MANIFEST_TOO_LARGE', 'bounded manifest required')
    raw = manifest_path.read_bytes()
    task = json.loads(raw)['spec']['id']
    job = os.environ.get('SLURM_JOB_ID', '')
    if not job.isdigit() or task != os.environ.get('AUTONOMICS_TASK_ID'):
        raise WorkerAdmissionError('SOFTWARE_TASK_MISMATCH', 'software evidence task differs')
    program = shutil.which(request['program'])
    if program is None:
        raise WorkerAdmissionError('RUNTIME_EXECUTABLE_MISSING', 'runtime executable unavailable')
    return {'schema': 'APPTAINER_WORKER_SOFTWARE_V1', 'worker_kind': 'python-apptainer',
            'task_id': task, 'job_id': job, 'manifest_sha256': hashlib.sha256(raw).hexdigest(),
            'request_sha256': sha(regular(root, 'container-request.json')),
            'worker_script': measured_software_file(__file__),
            # /proc/self/exe identifies the running interpreter, including a
            # deployment that invoked it through a mutable symlink.
            'interpreter': measured_software_file('/proc/self/exe'),
            'container_runtime': measured_software_file(program), 'sif': None}


def main():
    root = Path.cwd()
    diagnostics = WorkerDiagnostics()
    node_path = None
    heartbeat = None
    try:
        diagnostics.phase = 'request_admission'
        request = json.loads(regular(root, 'container-request.json').read_bytes())
        diagnostics.phase = 'allocation_admission'
        job = os.environ.get('SLURM_JOB_ID', '')
        cgroup = Path('/proc/self/cgroup').read_text()
        if (not job.isdigit() or not any(('job_' + job + '/') in line or ('job-' + job + '/') in line
                                        for line in cgroup.splitlines())):
            raise WorkerAdmissionError('SLURM_ALLOCATION_MISMATCH', 'worker is outside its recorded Slurm allocation')
        diagnostics.phase = 'software_identity'
        diagnostics.software = software_evidence(root, request)
        try:
            # Only the foreground exec helper enters CLONE_NEWUSER. Keep
            # coordinator telemetry live during input/image admission too.
            heartbeat = WorkerHeartbeat(root)
        except (OSError, ValueError, TypeError, KeyError):
            pass  # Advisory sensor failure is exposed as unknown by the reader.
        with node_workspace(root, request, diagnostics) as work:
            node_path = str(work)
            result = execute(root, request, work_root=work, heartbeat=heartbeat, diagnostics=diagnostics)
        result['node_workspace'] = {'path': node_path,
            'state': 'CLEANED_AFTER_TOOL_WAIT_AND_MEASURED_EXPORT',
            'allocation_stop_proved': False}
    except Exception as error:
        # Export only fixed codes and phases. Exception text/tracebacks may
        # contain credentials, private paths, or arbitrary payload content.
        failure = diagnostics.failure(error)
        result = {'schema': 'APPTAINER_CONTAINER_RESULT_V1', 'exit_code': 125,
                  'stdout': '', 'stderr': 'Apptainer worker failed: phase=%s code=%s' %
                  (failure['phase'], failure['code']), 'failure': failure, 'files': []}
        if node_path is not None:
            result['node_workspace'] = {'path': node_path,
                'state': 'RECOVERY_REQUIRED_NO_CLEANUP_INFERRED',
                'allocation_stop_proved': False}
        if diagnostics.software is not None:
            result['worker_software'] = diagnostics.software
        write_new(root / 'container-result.json', result)
        return
    finally:
        if heartbeat is not None:
            heartbeat.close()
    result['worker_software'] = diagnostics.software
    write_new(root / 'container-result.json', result)
    # The Slurm process succeeds when the supervisor produced a valid result.
    # The actual tool exit is returned to the original container node unchanged.


if __name__ == '__main__':
    if sys.argv[1:2] == ['--private-shm-exec']:
        raise SystemExit(private_shm_exec(sys.argv[2:]))
    main()
