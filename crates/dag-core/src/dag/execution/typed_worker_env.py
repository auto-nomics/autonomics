"""Owned credential-file trampoline; never place values in argv or receipts."""

import json
import hashlib
import fcntl
import os
import re
import stat
import sys

NETWORK_PATH = 'AUTONOMICS_NETWORK_ALLOWLIST'
NETWORK_HASH = 'AUTONOMICS_NETWORK_ALLOWLIST_SHA256'
ALLOWED = frozenset(('OPENALEX_API_KEY', 'S2_API_KEY', 'OPENGWAS_JWT', 'OPENGWAS_TOKEN',
                     NETWORK_PATH, NETWORK_HASH))


def identity(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid,
            info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError('duplicate private environment name')
        value[key] = item
    return value


def read_owned_file(path, limit, private):
    if not path.startswith('/') or any(p in ('', '.', '..') for p in path[1:].split('/')):
        raise ValueError('canonical private environment path required')
    parts = path[1:].split('/')
    parent = os.open('/', os.O_PATH | os.O_DIRECTORY)
    try:
        for part in parts[:-1]:
            child = os.open(part, os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
            os.close(parent)
            parent = child
        fd = os.open(parts[-1], os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW, dir_fd=parent)
        try:
            before = os.fstat(fd)
            if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.geteuid()
                    or (stat.S_IMODE(before.st_mode) != 0o600 if private else before.st_mode & 0o022)
                    or before.st_nlink != 1
                    or not (1 if private else 0) <= before.st_size <= limit):
                raise ValueError('private environment ownership or size rejected')
            data = bytearray()
            while len(data) <= limit:
                block = os.read(fd, min(65536, limit + 1 - len(data)))
                if not block:
                    break
                data.extend(block)
            if (len(data) != before.st_size or identity(os.fstat(fd)) != identity(before)
                    or identity(os.stat(parts[-1], dir_fd=parent, follow_symlinks=False)) != identity(before)):
                raise ValueError('private environment changed')
        finally:
            os.close(fd)
    finally:
        os.close(parent)
    return bytes(data)


def read_environment(path):
    data = read_owned_file(path, 65536, private=True)
    values = json.loads(data, object_pairs_hook=unique_object)
    if (not isinstance(values, dict) or not set(values) <= ALLOWED
            or any(not isinstance(value, str) or not value or not value.isascii()
                   or any(ord(c) < 32 or ord(c) == 127 for c in value)
                   for value in values.values())):
        raise ValueError('private environment schema rejected')
    if (NETWORK_PATH in values) != (NETWORK_HASH in values):
        raise ValueError('network policy requires both path and SHA256')
    if NETWORK_HASH in values and re.fullmatch(r'[0-9a-f]{64}', values[NETWORK_HASH]) is None:
        raise ValueError('network policy requires exact SHA256')
    return values


def network_policy_snapshot(values):
    """Pass only the measured policy bytes across exec, never a mutable path.

    This is operator configuration, not an agent-controlled environment key.
    A sealed inherited memfd closes the check/exec race without placing policy
    or credentials in argv, task JSON, or a shared temporary directory.
    """
    if NETWORK_PATH not in values:
        return None
    raw = read_owned_file(values[NETWORK_PATH], 1024 * 1024, private=False)
    if hashlib.sha256(raw).hexdigest() != values[NETWORK_HASH]:
        raise ValueError('network policy SHA256 mismatch')
    descriptor = os.memfd_create('autonomics-network-policy', os.MFD_ALLOW_SEALING)
    try:
        with os.fdopen(os.dup(descriptor), 'wb') as stream:
            stream.write(raw)
        os.lseek(descriptor, 0, os.SEEK_SET)
        fcntl.fcntl(descriptor, fcntl.F_ADD_SEALS,
                    fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_SEAL)
        os.set_inheritable(descriptor, True)
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def main():
    policy_fd = None
    try:
        values = read_environment(sys.argv[1])
        arguments = sys.argv[2:]
        if arguments[:1] == ['--network-policy-sha256']:
            if (len(arguments) < 3 or re.fullmatch(r'[0-9a-fA-F]{64}', arguments[1]) is None
                    or values.get(NETWORK_HASH) != arguments[1].lower()):
                raise ValueError('configured worker network policy is missing or differs')
            arguments = arguments[2:]
        if not arguments or not arguments[0].startswith('/'):
            raise ValueError('absolute measured worker required')
        environment = dict(os.environ)
        # A scheduler's ambient path is not a configured worker policy.
        environment.pop(NETWORK_PATH, None)
        environment.pop(NETWORK_HASH, None)
        environment.update({key: value for key, value in values.items()
                            if key not in (NETWORK_PATH, NETWORK_HASH)})
        policy_fd = network_policy_snapshot(values)
        if policy_fd is not None:
            environment[NETWORK_PATH] = '/proc/self/fd/' + str(policy_fd)
        os.execve(arguments[0], arguments, environment)
    except Exception:
        # JSON errors, invalid values and OS exceptions must not disclose keys.
        sys.stderr.write('typed worker private environment rejected\n')
        raise SystemExit(78)
    finally:
        if policy_fd is not None:
            os.close(policy_fd)


if __name__ == '__main__':
    main()
