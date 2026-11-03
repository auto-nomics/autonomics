"""Bounded, read-only SSH VFS protocol. No local cache or remote mutation.

Executed with Python's isolated mode on the SSH host. Requests travel over
stdin, never a shell command; responses contain a JSON header and optional
binary body. All path components are opened without following symlinks.
"""
import hashlib
import json
import os
import stat
import sys

CHUNK = 1024 * 1024
MAX_ENTRIES = 4096


class UnsupportedRuntimeError(Exception):
    pass


def validate_runtime():
    # Never substitute pathname listing or /proc fd symlinks: the held-fd,
    # bounded iterator is part of the confinement contract.
    if (sys.version_info < (3, 7)
            or not all(hasattr(os, name) for name in
                       ('pread', 'fstatvfs', 'O_NOFOLLOW', 'O_DIRECTORY', 'O_PATH', 'O_NONBLOCK'))
            or os.open not in os.supports_dir_fd
            or os.scandir not in os.supports_fd):
        raise UnsupportedRuntimeError('Python 3.7+ with confined fd operations required')


def filesystem_id(descriptor):
    try:
        fsid = os.fstatvfs(descriptor).f_fsid
    except (AttributeError, NotImplementedError, OSError):
        raise UnsupportedRuntimeError('held-fd filesystem identity unavailable')
    if type(fsid) is not int or not 0 < fsid <= (1 << 64) - 1:
        raise UnsupportedRuntimeError('nonzero filesystem identity required')
    return fsid


def identity(descriptor, info):
    # st_dev is a host-local mount number: GPFS can expose the same inode via
    # different device numbers on hosts behind one load-balanced SSH alias.
    # Keep a filesystem identity, measured on this held fd, in the remote pin.
    return {'kind': 'fsid-inode-v1', 'fsid': filesystem_id(descriptor),
            'inode': info.st_ino, 'size': info.st_size,
            'mtime_ns': info.st_mtime_ns, 'ctime_ns': info.st_ctime_ns}


def open_confined(root, key):
    if not isinstance(root, str) or not root.startswith('/') or root == '/':
        raise PermissionError('root')
    if os.path.realpath(root) != root.rstrip('/'):
        raise PermissionError('canonical root required')
    parts = key.strip('/').split('/') if key.strip('/') else []
    if any(part in ('', '.', '..') or '\x00' in part for part in parts):
        raise PermissionError('key')
    descriptor = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        # Open the root's ancestors too: a concurrent symlink replacement
        # between realpath validation and open must not change the data home.
        root_parts = root.strip('/').split('/')
        for index, part in enumerate(root_parts):
            # GPFS ancestors can permit search without directory listing.
            access = os.O_RDONLY if index == len(root_parts) - 1 else os.O_PATH
            following = os.open(part, access | os.O_DIRECTORY | os.O_NOFOLLOW,
                                dir_fd=descriptor)
            os.close(descriptor)
            descriptor = following
        for index, part in enumerate(parts):
            if index < len(parts) - 1:
                flags = os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW
            else:
                flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
            following = os.open(part, flags, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = following
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def metadata(descriptor, info):
    if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):
        raise PermissionError('regular files and directories only')
    return {'directory': stat.S_ISDIR(info.st_mode), 'size': info.st_size,
            'modified': info.st_mtime_ns // 1000000, 'identity': identity(descriptor, info)}


def execute(request):
    validate_runtime()
    descriptor = open_confined(request['root'], request['path'])
    try:
        before = os.fstat(descriptor)
        result = metadata(descriptor, before)
        operation = request['op']
        if operation == 'stat':
            return result, b''
        if operation == 'read':
            if not stat.S_ISREG(before.st_mode):
                raise IsADirectoryError()
            if request['identity'] != result['identity']:
                raise RuntimeError('changed')
            size, offset = request['size'], request['offset']
            if type(size) is not int or not 0 <= size <= CHUNK:
                raise ValueError('size')
            if type(offset) is not int or offset < 0:
                raise ValueError('offset')
            body = os.pread(descriptor, size, offset)
            after = os.fstat(descriptor)
            if (after.st_dev != before.st_dev
                    or identity(descriptor, after) != result['identity']):
                raise RuntimeError('changed')
            result.update(length=len(body), sha256=hashlib.sha256(body).hexdigest())
            return result, body
        if operation == 'list':
            if not stat.S_ISDIR(before.st_mode):
                raise NotADirectoryError()
            entries = []
            with os.scandir(descriptor) as listing:
                for entry in listing:
                    if len(entries) >= MAX_ENTRIES:
                        raise ValueError('directory limit')
                    # O_PATH obtains the entry's own filesystem identity even
                    # at a mount boundary, without granting content reads.
                    entry_fd = os.open(entry.name, os.O_PATH | os.O_NOFOLLOW,
                                       dir_fd=descriptor)
                    try:
                        info = os.fstat(entry_fd)
                        if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):
                            continue
                        entries.append(dict(metadata(entry_fd, info), name=entry.name))
                    finally:
                        os.close(entry_fd)
            result['entries'] = entries
            return result, b''
        raise ValueError('operation')
    finally:
        os.close(descriptor)


def main():
    try:
        wire = sys.stdin.buffer.read(65537)
        if len(wire) > 65536:
            raise ValueError('request limit')
        header, body = execute(json.loads(wire))
        header['ok'] = True
    except UnsupportedRuntimeError:
        header, body = {'ok': False, 'error': 'unsupported_runtime'}, b''
    except FileNotFoundError:
        header, body = {'ok': False, 'error': 'not_found'}, b''
    except (PermissionError, OSError):
        header, body = {'ok': False, 'error': 'permission_denied'}, b''
    except RuntimeError:
        header, body = {'ok': False, 'error': 'changed'}, b''
    except Exception:
        header, body = {'ok': False, 'error': 'invalid_request'}, b''
    sys.stdout.buffer.write(json.dumps(header, separators=(',', ':')).encode() + b'\n')
    sys.stdout.buffer.write(body)


if __name__ == '__main__':
    main()
