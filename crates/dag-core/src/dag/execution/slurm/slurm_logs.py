"""Bounded snapshots of one stopped Slurm attempt's logs; no job execution."""
import hashlib
import json
import os
import stat
import sys

LIMIT = 64 * 1024


def identity(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size,
            info.st_mtime_ns, info.st_ctime_ns)


def capture(root, directory, job, stream, suffix):
    name = 'slurm-' + job + '.' + suffix
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
    except FileNotFoundError:
        return None
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode):
            raise ValueError('Slurm log is not a regular file')
        truncated = before.st_size > LIMIT
        os.lseek(fd, max(0, before.st_size - LIMIT), os.SEEK_SET)
        chunks = []
        remaining = min(before.st_size, LIMIT)
        while remaining:
            chunk = os.read(fd, remaining)
            if not chunk:
                raise ValueError('Slurm log shortened during capture')
            chunks.append(chunk)
            remaining -= len(chunk)
        if (identity(before) != identity(os.fstat(fd)) or
                identity(before) != identity(os.stat(name, dir_fd=directory, follow_symlinks=False))):
            raise ValueError('Slurm log changed during capture')
    finally:
        os.close(fd)
    data = (b'[truncated: final 65536 bytes follow]\n' if truncated else b'') + b''.join(chunks)
    target = 'transport-' + stream + '.log'
    try:
        out = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                      0o600, dir_fd=directory)
    except FileExistsError:
        # A repeated read after an interrupted collection must agree byte for byte.
        out = os.open(target, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        with os.fdopen(out, 'rb') as saved:
            if not stat.S_ISREG(os.fstat(saved.fileno()).st_mode) or saved.read(LIMIT + 100) != data:
                raise ValueError('existing log snapshot differs')
    else:
        with os.fdopen(out, 'wb') as saved:
            saved.write(data)
            saved.flush()
            os.fsync(saved.fileno())
    return {'path': root + '/' + target, 'format': 'text',
            'fingerprint': {'size': len(data), 'mtime_ns': 0,
                            'content_hash': hashlib.sha256(data).hexdigest(),
                            'immutable_remote': False}}


def main():
    root, job = sys.argv[1:]
    # Login nodes may still provide Python 3.6 (str.isascii arrived in 3.7).
    if not root.startswith('/') or not job or any(c < '0' or c > '9' for c in job):
        raise ValueError('invalid Slurm log identity')
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        print(json.dumps({'stdout': capture(root, directory, job, 'stdout', 'out'),
                          'stderr': capture(root, directory, job, 'stderr', 'err')}))
    finally:
        os.close(directory)


if __name__ == '__main__':
    main()
