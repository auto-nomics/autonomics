"""Bounded SSH VFS write protocol: staged writes with atomic rename, plus
delete, rename and mkdir.

Executed with Python's isolated mode on the SSH host. The request header
travels as one JSON line on stdin — never a shell command — and write
bodies follow as raw bytes until EOF. All path components are resolved
without following symlinks; staged files are created inside the target
directory so the final rename is atomic on the same filesystem.

An aborted or interrupted write leaves its staged file (prefix
``.autonomics-staging-``) behind instead of touching the destination:
a torn destination can never be observed.
"""
import hashlib
import json
import os
import stat
import sys
import uuid

CHUNK = 1024 * 1024
STAGING_PREFIX = '.autonomics-staging-'


class UnsupportedRuntimeError(Exception):
    pass


def validate_runtime():
    if (sys.version_info < (3, 7)
            or not all(hasattr(os, name) for name in
                       ('O_NOFOLLOW', 'O_DIRECTORY', 'O_PATH', 'replace'))
            or os.open not in os.supports_dir_fd
            or os.rename not in os.supports_dir_fd
            or os.unlink not in os.supports_dir_fd):
        raise UnsupportedRuntimeError('Python 3.7+ with confined fd operations required')


def root_descriptor(root):
    if not isinstance(root, str) or not root.startswith('/') or root == '/':
        raise PermissionError('root')
    if os.path.realpath(root) != root.rstrip('/'):
        raise PermissionError('canonical root required')
    descriptor = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        # Open the root's ancestors too: a concurrent symlink replacement
        # between realpath validation and open must not change the data home.
        parts = root.strip('/').split('/')
        for index, part in enumerate(parts):
            access = os.O_RDONLY if index == len(parts) - 1 else os.O_PATH
            following = os.open(part, access | os.O_DIRECTORY | os.O_NOFOLLOW,
                                dir_fd=descriptor)
            os.close(descriptor)
            descriptor = following
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def confined_parent(root, key):
    """Open the parent directory of ``key`` under a held root fd and return
    ``(dir_fd, final_name)``. Intermediate components never follow symlinks."""
    parts = key.strip('/').split('/') if key.strip('/') else []
    if not parts or any(part in ('', '.', '..') or '\x00' in part for part in parts):
        raise PermissionError('key')
    final = parts[-1]
    if final.startswith(STAGING_PREFIX):
        raise PermissionError('staging namespace is reserved')
    descriptor = root_descriptor(root)
    try:
        for part in parts[:-1]:
            following = os.open(part, os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW,
                                dir_fd=descriptor)
            os.close(descriptor)
            descriptor = following
        probe = os.fstat(descriptor)
        if not stat.S_ISDIR(probe.st_mode):
            raise NotADirectoryError()
        return descriptor, final
    except BaseException:
        os.close(descriptor)
        raise


def frames(stream):
    """Yield framed body chunks. Each frame is ``{len:x}\\n`` followed by
    exactly that many bytes; a zero-length frame terminates the body. A
    stream that ends without the terminator is an aborted write — the
    caller must not commit — so a truncated upload can never be renamed
    into place by a plain EOF."""
    while True:
        line = stream.readline(16)
        if not line:
            raise EOFError('stream ended without a commit frame')
        size = int(line, 16)
        if size == 0:
            return
        remaining = size
        while remaining:
            chunk = stream.read(min(remaining, CHUNK))
            if not chunk:
                raise EOFError('truncated frame')
            remaining -= len(chunk)
            yield chunk


def op_write(request, body):
    parent, name = confined_parent(request['root'], request['path'])
    try:
        staged = '%s%s' % (STAGING_PREFIX, uuid.uuid4().hex)
        descriptor = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                             0o644, dir_fd=parent)
        try:
            digest = hashlib.sha256()
            written = 0
            for chunk in frames(body):
                digest.update(chunk)
                view = memoryview(chunk)
                while view:
                    count = os.write(descriptor, view)
                    written += count
                    view = view[count:]
            os.fsync(descriptor)
            os.fchmod(descriptor, 0o644)
            # The rename stays inside the held directory, so the destination
            # can never be swapped for a symlink mid-operation.
            os.replace(staged, name, src_dir_fd=parent, dst_dir_fd=parent)
        except BaseException:
            try:
                os.unlink(staged, dir_fd=parent)
            except OSError:
                pass
            raise
        finally:
            os.close(descriptor)
        return {'size': written, 'sha256': digest.hexdigest()}
    finally:
        os.close(parent)


def op_delete(request):
    parent, name = confined_parent(request['root'], request['path'])
    try:
        probe = os.open(name, os.O_PATH | os.O_NOFOLLOW, dir_fd=parent)
        try:
            info = os.fstat(probe)
        finally:
            os.close(probe)
        if stat.S_ISDIR(info.st_mode):
            os.rmdir(name, dir_fd=parent)
        else:
            os.unlink(name, dir_fd=parent)
    finally:
        os.close(parent)
    return {}


def op_rename(request):
    source_parent, source = confined_parent(request['root'], request['from'])
    try:
        destination_parent, destination = confined_parent(request['root'], request['to'])
        try:
            os.replace(source, destination,
                       src_dir_fd=source_parent, dst_dir_fd=destination_parent)
        finally:
            os.close(destination_parent)
    finally:
        os.close(source_parent)
    return {}


def op_mkdir(request):
    parent, name = confined_parent(request['root'], request['path'])
    try:
        os.mkdir(name, 0o755, dir_fd=parent)
    finally:
        os.close(parent)
    return {}


def execute(request):
    validate_runtime()
    operation = request['op']
    if operation == 'write':
        return op_write(request, sys.stdin.buffer)
    if operation == 'delete':
        return op_delete(request)
    if operation == 'rename':
        return op_rename(request)
    if operation == 'mkdir':
        return op_mkdir(request)
    raise ValueError('operation')


def main():
    try:
        line = sys.stdin.buffer.readline(65537)
        if len(line) > 65536:
            raise ValueError('request limit')
        header = json.loads(line)
        header.update(execute(header) or {})
        header['ok'] = True
    except UnsupportedRuntimeError:
        header = {'ok': False, 'error': 'unsupported_runtime'}
    except FileNotFoundError:
        header = {'ok': False, 'error': 'not_found'}
    except NotADirectoryError:
        header = {'ok': False, 'error': 'not_a_directory'}
    except (PermissionError, OSError):
        header = {'ok': False, 'error': 'permission_denied'}
    except Exception:
        header = {'ok': False, 'error': 'invalid_request'}
    sys.stdout.buffer.write(json.dumps(header, separators=(',', ':')).encode() + b'\n')


if __name__ == '__main__':
    main()
