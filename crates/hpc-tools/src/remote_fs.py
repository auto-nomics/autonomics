"""Account filesystem helper; Python 3.6+, stdlib only, no shell commands.

Executed by the fixed stdin-envelope bootstrap with REQUEST set. It may also
be imported by toy tests. Incomplete uploads are durable named account files;
they are never treated as completed results or automatically expired.
"""
import base64
import ctypes
import fcntl
import hashlib
import json
import os
import re
import shutil
import stat
import tempfile
import uuid

CHUNK = 1024 * 1024
MAX_WRITE = 4 * CHUNK
HEX = re.compile(r"^[0-9a-f]{64}$")
UPLOAD = re.compile(r"^[0-9a-f]{32}$")


def integer(value, default, maximum):
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > maximum:
        raise ValueError("integer is outside its permitted range")
    return value


def path(value):
    if not isinstance(value, str) or not value or "\x00" in value:
        raise ValueError("a nonempty remote path is required")
    value = os.path.expanduser(value)
    if not os.path.isabs(value):
        value = os.path.join(os.path.expanduser("~"), value)
    return os.path.normpath(value)


def digest(fd):
    h = hashlib.sha256()
    os.lseek(fd, 0, os.SEEK_SET)
    while True:
        chunk = os.read(fd, CHUNK)
        if not chunk:
            break
        h.update(chunk)
    return h.hexdigest()


def regular_fd(target, write=False):
    fd = os.open(target, (os.O_RDWR if write else os.O_RDONLY) | os.O_NOFOLLOW | os.O_NONBLOCK)
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise ValueError("operation requires a regular file, not a link or special device")
    return fd


def identity(st):
    return (st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns, st.st_ctime_ns)


def summary(target):
    st = os.lstat(target)
    kind = "directory" if stat.S_ISDIR(st.st_mode) else "file" if stat.S_ISREG(st.st_mode) else "symlink" if stat.S_ISLNK(st.st_mode) else "special"
    result = {"path": target, "name": os.path.basename(target), "kind": kind,
              "size": st.st_size, "mode": oct(stat.S_IMODE(st.st_mode)),
              "uid": st.st_uid, "gid": st.st_gid, "mtime_ns": st.st_mtime_ns}
    if kind == "symlink":
        result["link_target"] = os.readlink(target)
    return result


def content(request, maximum=MAX_WRITE):
    value = request.get("content")
    if not isinstance(value, str):
        raise ValueError("content must be text or base64 text")
    encoding = request.get("encoding") or "utf8"
    if encoding == "base64":
        data = base64.b64decode(value.encode("ascii"), validate=True)
    elif encoding == "utf8":
        data = value.encode("utf-8")
    else:
        raise ValueError("encoding must be utf8 or base64")
    if len(data) > maximum:
        raise ValueError("content exceeds chunk limit")
    return data


def expected_existing(target, expected):
    if not isinstance(expected, str) or not HEX.fullmatch(expected):
        raise ValueError("expected_sha256 must be 64 lowercase hexadecimal characters; omit it or use JSON null for a new file")
    fd = regular_fd(target)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        before = os.fstat(fd)
        if digest(fd) != expected or identity(before) != identity(os.fstat(fd)):
            raise ValueError("existing destination digest changed")
        if identity(before) != identity(os.lstat(target)):
            raise ValueError("existing destination path changed")
        return fd, before
    except BaseException:
        os.close(fd)
        raise


def commit_file(part, target, expected):
    """No-clobber is atomic. Explicit replacement uses cooperative advisory
    locking plus inode/hash rechecks, not a filesystem-wide compare-and-swap.
    """
    parent_fd = os.open(os.path.dirname(target), os.O_RDONLY | os.O_DIRECTORY)
    try:
        if expected is None:
            os.link(part, target, follow_symlinks=False)
            os.unlink(part)
        else:
            fd, before = expected_existing(target, expected)
            try:
                if identity(before) != identity(os.lstat(target)):
                    raise ValueError("destination changed before replacement")
                os.replace(part, target)
            finally:
                os.close(fd)
        os.fsync(parent_fd)
    finally:
        os.close(parent_fd)


class DirectoryPublishError(ValueError):
    def __init__(self, message, details):
        super().__init__(message)
        self.details = details


def inode(st):
    return (st.st_dev, st.st_ino)


def cooperative_directory_publish(source, destination):
    """Portable publication for explicitly cooperating account processes.

    This never claims kernel no-replace or CAS against outside same-UID writers.
    A failed reservation is retained rather than racing to unlink a potentially
    substituted directory. The caller can inspect it and explicitly remove it.
    """
    parent = os.path.dirname(destination)
    name = os.path.basename(destination)
    flags = getattr(os, "O_PATH", os.O_RDONLY) | os.O_DIRECTORY | os.O_NOFOLLOW
    parent_fd = os.open(parent, flags)
    source_fd = None
    lock_fd = None
    reservation_fd = None
    reservation_identity = None
    reservation_created = False
    rename_attempted = False
    lock_name = ".autonomics-directory-publish.lock"
    try:
        parent_identity = inode(os.fstat(parent_fd))
        source_fd = os.open(source, flags)
        source_identity = inode(os.fstat(source_fd))
        lock_fd = os.open(lock_name, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600, dir_fd=parent_fd)
        lock_stat = os.fstat(lock_fd)
        if not stat.S_ISREG(lock_stat.st_mode) or lock_stat.st_uid != os.getuid() or stat.S_IMODE(lock_stat.st_mode) != 0o600:
            raise ValueError("directory publication lock must be an owned 0600 regular file")
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        if inode(lock_stat) != inode(os.stat(lock_name, dir_fd=parent_fd, follow_symlinks=False)):
            raise ValueError("directory publication lock path changed")
        if parent_identity != inode(os.stat(parent, follow_symlinks=False)):
            raise ValueError("directory publication parent path changed")
        # Existing directories (including empty ones), links, and files all
        # reject here. This reservation is the only name we may replace.
        os.mkdir(name, mode=0o700, dir_fd=parent_fd)
        reservation_created = True
        reservation_fd = os.open(name, flags, dir_fd=parent_fd)
        reservation_identity = inode(os.fstat(reservation_fd))
        if inode(os.stat(name, dir_fd=parent_fd, follow_symlinks=False)) != reservation_identity:
            raise ValueError("directory reservation path changed")
        if source_identity != inode(os.stat(source, follow_symlinks=False)):
            raise ValueError("directory publication source path changed")
        if parent_identity != inode(os.stat(parent, follow_symlinks=False)):
            raise ValueError("directory publication parent changed before rename")
        if inode(lock_stat) != inode(os.stat(lock_name, dir_fd=parent_fd, follow_symlinks=False)):
            raise ValueError("directory publication lock changed before rename")
        # Kernel rename refuses a nonempty target directory. The pinned empty
        # reservation and private advisory lock serialize cooperating clients.
        if inode(os.stat(name, dir_fd=parent_fd, follow_symlinks=False)) != reservation_identity:
            raise ValueError("directory reservation changed before rename")
        rename_attempted = True
        os.rename(source, name, dst_dir_fd=parent_fd)
        if source_identity != inode(os.stat(name, dir_fd=parent_fd, follow_symlinks=False)) or parent_identity != inode(os.stat(parent, follow_symlinks=False)):
            raise ValueError("directory publication identity could not be confirmed")
        return {"directory_publish_mode": "cooperative_reservation", "atomic_noreplace": False,
                "cooperative_guard": True, "external_same_uid_cas": False,
                "publication_identity_verified": True}
    except (OSError, ValueError) as error:
        if reservation_created:
            # Never automatically unlink on an exception: a same-UID writer
            # could swap even an empty directory between an inode check and
            # rmdir, which has no conditional-inode form on these filesystems.
            retained = None
            if reservation_identity is not None:
                try:
                    retained = inode(os.stat(name, dir_fd=parent_fd, follow_symlinks=False)) == reservation_identity
                except FileNotFoundError:
                    retained = False
                except OSError:
                    retained = None
            raise DirectoryPublishError(str(error), {
                "directory_publish_mode": "cooperative_reservation", "atomic_noreplace": False,
                "cooperative_guard": True, "external_same_uid_cas": False,
                "reservation_retained": retained, "reservation_cleanup_attempted": False,
                "destination_requires_inspection": True,
                "mutation_may_have_completed": rename_attempted, "destination": destination})
        raise
    finally:
        for fd in (reservation_fd, lock_fd, source_fd, parent_fd):
            if fd is not None:
                os.close(fd)


def rename_no_replace(source, destination, mode="strict"):
    if mode == "cooperative_reservation":
        return cooperative_directory_publish(source, destination)
    if mode != "strict":
        raise ValueError("directory_publish_mode must be strict or cooperative_reservation")
    libc = ctypes.CDLL(None, use_errno=True)
    rename = getattr(libc, "renameat2", None)
    if rename is None:
        raise ValueError("server lacks atomic no-clobber directory rename")
    rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    rename.restype = ctypes.c_int
    if rename(-100, os.fsencode(source), -100, os.fsencode(destination), 1) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), destination)
    return {"directory_publish_mode": "strict", "atomic_noreplace": True,
            "cooperative_guard": False}


def staged_write(target, data, expected):
    fd, part = tempfile.mkstemp(prefix=".autonomics-write-", dir=os.path.dirname(target))
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        commit_file(part, target, expected)
    finally:
        if os.path.lexists(part):
            os.unlink(part)
    return {"path": target, "size": len(data), "sha256": hashlib.sha256(data).hexdigest(),
            "replacement_locking": "cooperative_advisory" if expected else "atomic_no_clobber"}


def upload_paths(target, handle):
    if not isinstance(handle, str) or not UPLOAD.fullmatch(handle):
        raise ValueError("invalid upload_id")
    stem = os.path.join(os.path.dirname(target), ".autonomics-upload-" + handle)
    return stem + ".part", stem + ".json"


def upload_metadata(target, handle):
    part, metadata = upload_paths(target, handle)
    fd = regular_fd(metadata)
    try:
        st = os.fstat(fd)
        if st.st_uid != os.getuid() or stat.S_IMODE(st.st_mode) != 0o600 or st.st_size > 8192:
            raise ValueError("upload metadata identity or permissions changed")
        data = os.read(fd, 8193)
        if identity(st) != identity(os.fstat(fd)) or identity(st) != identity(os.lstat(metadata)):
            raise ValueError("upload metadata changed while reading")
    finally:
        os.close(fd)
    record = json.loads(data.decode("utf8"))
    if not isinstance(record, dict):
        raise ValueError("invalid upload metadata")
    if record.get("path") != target or record.get("upload_id") != handle:
        raise ValueError("upload handle is bound to a different destination")
    st = os.lstat(part)
    if record.get("part_identity") != [st.st_dev, st.st_ino] or not stat.S_ISREG(st.st_mode):
        raise ValueError("upload staging inode changed")
    return part, metadata, record


def do_upload(request, target):
    action = request["action"]
    if action == "upload_begin":
        handle = uuid.uuid4().hex
        part, metadata = upload_paths(target, handle)
        fd = os.open(part, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        os.close(fd)
        st = os.lstat(part)
        record = {"path": target, "upload_id": handle, "expected_sha256": request.get("expected_sha256"),
                  "part_identity": [st.st_dev, st.st_ino]}
        try:
            fd = os.open(metadata, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, "w") as stream:
                json.dump(record, stream)
                stream.flush()
                os.fsync(stream.fileno())
        except BaseException:
            os.unlink(part)
            raise
        return {"path": target, "upload_id": handle, "offset": 0, "chunk_limit": CHUNK}
    handle = request.get("upload_id")
    try:
        part, metadata, record = upload_metadata(target, handle)
    except FileNotFoundError:
        if action != "upload_commit":
            raise
        # An SSH acknowledgement or the final metadata unlink may have been
        # lost. Observe the current desired bytes without fabricating a
        # historical successful commit or attributing an unrelated file to us.
        upload_paths(target, handle)
        size = integer(request.get("size"), None, 2 ** 63 - 1)
        sha = request.get("sha256")
        if size is None or not isinstance(sha, str) or not HEX.fullmatch(sha):
            raise ValueError("commit state verification requires exact total size and sha256")
        fd = regular_fd(target)
        try:
            before = os.fstat(fd)
            if before.st_size != size or digest(fd) != sha:
                raise ValueError("upload handle is missing and current destination bytes differ")
            if identity(before) != identity(os.fstat(fd)) or identity(before) != identity(os.lstat(target)):
                raise ValueError("current destination changed during state verification")
        finally:
            os.close(fd)
        return {"path": target, "upload_id": handle, "size": size, "sha256": sha,
                "status": "CURRENT_TARGET_BYTES_VERIFIED", "already_present": True,
                "current_target_bytes_verified": True, "committed": False,
                "historical_commit_verified": False, "receipt_authenticity_verified": False}
    fd = regular_fd(part, write=True)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        # A cooperating chunk/commit/abort may have run while this request
        # waited for the inode lock. Refresh metadata and bind this opened FD
        # to its recorded inode, not merely to the current named path.
        _, _, current_record = upload_metadata(target, handle)
        if current_record != record:
            raise ValueError("upload metadata changed before staging lock")
        st = os.fstat(fd)
        if st.st_uid != os.getuid() or stat.S_IMODE(st.st_mode) != 0o600 or [st.st_dev, st.st_ino] != record.get("part_identity"):
            raise ValueError("upload staging identity or permissions changed")
        if identity(st) != identity(os.lstat(part)):
            raise ValueError("upload staging path changed")
        if action == "upload_abort":
            # The same staging lock serializes abort with chunk and commit.
            # No recursive removal or caller-supplied staging path is used.
            metadata_identity = identity(os.lstat(metadata))
            if identity(st) != identity(os.fstat(fd)) or identity(st) != identity(os.lstat(part)):
                raise ValueError("upload staging changed before abort")
            os.unlink(part)
            if metadata_identity != identity(os.lstat(metadata)):
                raise ValueError("upload metadata changed during abort; metadata retained")
            os.unlink(metadata)
            return {"path": target, "upload_id": handle, "aborted": True}
        if action == "upload_status":
            return {"path": target, "upload_id": handle, "size": st.st_size, "offset": st.st_size,
                    "staging_identity": list(identity(st)), "chunk_limit": CHUNK,
                    "expected_sha256": record.get("expected_sha256"), "committed": False}
        if action == "upload_chunk":
            data = content(request, CHUNK)
            offset = integer(request.get("offset"), 0, 2 ** 63 - 1)
            sha = request.get("sha256")
            actual = hashlib.sha256(data).hexdigest()
            if sha != actual:
                raise ValueError("upload chunk sha256 is required and must match")
            # Idempotent retry after an uncertain SSH acknowledgement.
            if offset < st.st_size:
                os.lseek(fd, offset, os.SEEK_SET)
                if os.read(fd, len(data)) == data:
                    return {"path": target, "upload_id": handle, "offset": st.st_size, "chunk_sha256": actual, "replayed": True}
                raise ValueError("upload retry differs from already stored bytes")
            if offset != st.st_size:
                raise ValueError("upload offset does not match staged length")
            os.lseek(fd, offset, os.SEEK_SET)
            view = memoryview(data)
            while view:
                written = os.write(fd, view)
                if written <= 0:
                    raise IOError("short upload write")
                view = view[written:]
            os.fsync(fd)
            return {"path": target, "upload_id": handle, "offset": offset + len(data), "chunk_sha256": actual}
        if action != "upload_commit":
            raise ValueError("unknown upload action")
        size = integer(request.get("size"), None, 2 ** 63 - 1)
        sha = request.get("sha256")
        if size is None or size != st.st_size or not isinstance(sha, str) or not HEX.fullmatch(sha):
            raise ValueError("upload commit requires exact total size and sha256")
        actual = digest(fd)
        if actual != sha or identity(st) != identity(os.fstat(fd)) or identity(st) != identity(os.lstat(part)):
            raise ValueError("upload full digest or identity verification failed")
        os.fsync(fd)
        commit_file(part, target, record.get("expected_sha256"))
        os.unlink(metadata)
        return {"path": target, "upload_id": handle, "size": size, "sha256": actual, "committed": True}
    finally:
        os.close(fd)


def operation(request):
    if not isinstance(request, dict) or not isinstance(request.get("action"), str):
        raise ValueError("filesystem request requires a string action")
    # A quoted JSON null is a real string. Never turn it into a home-relative
    # destination or silently discard a replacement/content identity guard.
    invalid = [field for field in ("path", "destination", "encoding", "cursor", "directory_publish_mode",
                                  "expected_sha256", "sha256", "upload_id")
               if request.get(field) in ("", "null")]
    if invalid:
        raise ValueError(', '.join(invalid) + ' must not be empty strings or the string "null"; '
                         'omit unused optional fields or use JSON null (without quotes)')
    if request["action"].startswith("upload_") and request.get("path") is None:
        raise ValueError("upload actions require the original destination path returned by upload_begin; upload_id alone is insufficient")
    for field, pattern in (("expected_sha256", HEX), ("sha256", HEX), ("upload_id", UPLOAD)):
        if request.get(field) is not None and (not isinstance(request[field], str) or not pattern.fullmatch(request[field])):
            raise ValueError(field + " has an invalid format; omit unused optional fields or use JSON null")
    action = request.get("action")
    if action == "info":
        return {"home": os.path.expanduser("~"), "uid": os.getuid(), "gid": os.getgid(),
                "groups": os.getgroups(), "cwd": os.getcwd(), "hostname": os.uname().nodename,
                "permissions": "authenticated_account_posix", "chunk_limit": CHUNK}
    target = path(request.get("path"))
    if action.startswith("upload_"):
        return do_upload(request, target)
    if action == "stat":
        result = summary(target)
        if result["kind"] == "file":
            # Hashing is explicit to avoid scanning GB files for plain stat.
            result["sha256"] = None
        return result
    if action == "list":
        limit = integer(request.get("limit"), 100, 500)
        if not limit:
            raise ValueError("listing limit must be positive")
        start = integer(request.get("page"), 0, 10 ** 9)
        # Bound memory even on very large account directories. The opaque
        # directory order can change when writers mutate it between pages.
        entries = []
        seen = 0
        more = False
        with os.scandir(target) as iterator:
            for entry in iterator:
                if seen < start:
                    seen += 1
                    continue
                if len(entries) >= limit:
                    more = True
                    break
                entries.append(summary(entry.path))
                seen += 1
        return {"path": target, "entries": entries, "next_page": seen if more else None,
                "snapshot_consistent": False}
    if action == "read":
        offset = integer(request.get("offset"), 0, 2 ** 63 - 1)
        limit = integer(request.get("limit"), 65536, CHUNK)
        fd = regular_fd(target)
        try:
            before = os.fstat(fd)
            if not limit and offset != before.st_size:
                raise ValueError("zero-length identity checks require the exact end-of-file offset")
            os.lseek(fd, offset, os.SEEK_SET)
            data = os.read(fd, limit)
            if identity(before) != identity(os.fstat(fd)) or identity(before) != identity(os.lstat(target)):
                raise ValueError("file changed while reading")
        finally:
            os.close(fd)
        encoding = request.get("encoding") or "utf8"
        if encoding == "base64":
            value = base64.b64encode(data).decode("ascii")
        elif encoding == "utf8":
            value = data.decode("utf8")
        else:
            raise ValueError("encoding must be utf8 or base64")
        return {"path": target, "offset": offset, "size": before.st_size,
                "bytes": len(data), "encoding": encoding, "content": value,
                "chunk_sha256": hashlib.sha256(data).hexdigest(),
                "identity": list(identity(before)), "next_offset": offset + len(data),
                "eof": offset + len(data) >= before.st_size}
    if action == "search":
        query = request.get("query") or ""
        if not isinstance(query, str) or len(query) > 1024:
            raise ValueError("search query must be a literal substring up to 1024 characters")
        limit = integer(request.get("limit"), 100, 500)
        if not limit:
            raise ValueError("search limit must be positive")
        cursor = request.get("cursor") or ""
        matches = []
        scanned = 0
        next_cursor = None
        # Lexical cursor is stable without a giant traversal-sized response;
        # bounded scan avoids monopolizing the data node.
        def visit(directory, depth=0):
            if depth > 64:
                raise ValueError("search depth exceeds 64; narrow the starting directory")
            with os.scandir(directory) as iterator:
                rows = []
                for row in iterator:
                    rows.append(row)
                    if len(rows) > 10000:
                        raise ValueError("search directory exceeds 10000 entries; use paginated list or narrow the path")
                rows.sort(key=lambda row: row.name)
            for row in rows:
                yield row.path
                if request.get("recursive") and row.is_dir(follow_symlinks=False):
                    for child in visit(row.path, depth + 1):
                        yield child
        for candidate in visit(target):
            if tuple(candidate.split(os.sep)) <= tuple(cursor.split(os.sep)):
                continue
            scanned += 1
            if query in candidate:
                matches.append(summary(candidate))
            if len(matches) >= limit or scanned >= 10000:
                next_cursor = candidate
                break
        return {"path": target, "matches": matches, "cursor": next_cursor,
                "scanned": scanned, "snapshot_consistent": False}
    if action == "mkdir":
        if request.get("recursive"):
            os.makedirs(target, mode=0o700, exist_ok=False)
        else:
            os.mkdir(target, 0o700)
        return summary(target)
    if action == "write":
        return staged_write(target, content(request), request.get("expected_sha256"))
    if action in ("copy", "move"):
        destination = path(request.get("destination"))
        source_stat = os.lstat(target)
        if stat.S_ISDIR(source_stat.st_mode):
            publish_mode = request.get("directory_publish_mode") or "strict"
            if target in ("/", os.path.expanduser("~")):
                raise ValueError("refusing copy/move of filesystem root or account home")
            if request.get("expected_sha256") is not None:
                raise ValueError("directory replacement is not supported; destination must not exist")
            # Copying into the source would recursively consume its own stage.
            if os.path.commonpath([os.path.realpath(target), os.path.realpath(destination)]) == os.path.realpath(target):
                raise ValueError("directory destination cannot be inside its source")
            if action == "move":
                publication = rename_no_replace(target, destination, publish_mode)
                return dict(publication, path=target, destination=destination, moved=True, kind="directory")
            if not request.get("recursive"):
                raise ValueError("directory copy requires recursive=true")
            stage = tempfile.mkdtemp(prefix=".autonomics-tree-", dir=os.path.dirname(destination))
            os.rmdir(stage)
            try:
                shutil.copytree(target, stage, symlinks=True)
                publication = rename_no_replace(stage, destination, publish_mode)
            finally:
                if os.path.lexists(stage):
                    shutil.rmtree(stage)
            return dict(publication, path=target, destination=destination, kind="directory",
                        source_snapshot_consistent=False, moved=False)
        fd = regular_fd(target)
        part = None
        try:
            before = os.fstat(fd)
            out_fd, part = tempfile.mkstemp(prefix=".autonomics-copy-", dir=os.path.dirname(destination))
            h = hashlib.sha256()
            with os.fdopen(out_fd, "wb") as out:
                while True:
                    data = os.read(fd, CHUNK)
                    if not data:
                        break
                    h.update(data)
                    out.write(data)
                out.flush()
                os.fsync(out.fileno())
            if identity(before) != identity(os.fstat(fd)) or identity(before) != identity(os.lstat(target)):
                raise ValueError("source changed during copy")
            commit_file(part, destination, request.get("expected_sha256"))
            if action == "move":
                if identity(before) != identity(os.lstat(target)):
                    raise ValueError("destination was copied but source changed; source retained")
                os.unlink(target)
            return {"path": target, "destination": destination, "size": before.st_size, "sha256": h.hexdigest(), "moved": action == "move"}
        finally:
            os.close(fd)
            if part and os.path.lexists(part):
                os.unlink(part)
    if action == "remove":
        if target in ("/", os.path.expanduser("~")):
            raise ValueError("refusing removal of filesystem root or account home")
        st = os.lstat(target)
        if stat.S_ISDIR(st.st_mode):
            if request.get("recursive"):
                shutil.rmtree(target)
            else:
                os.rmdir(target)
        else:
            expected = request.get("expected_sha256")
            fd = None
            if expected is not None:
                fd, before = expected_existing(target, expected)
                if identity(before) != identity(os.lstat(target)):
                    os.close(fd)
                    raise ValueError("removal target changed")
            try:
                os.unlink(target)
            finally:
                if fd is not None:
                    os.close(fd)
        return {"path": target, "removed": True}
    raise ValueError("unknown filesystem action")


def run(request):
    try:
        # Keep argument validation in operation(), before any filesystem work,
        # so callers receive the same actionable guidance for every bad field.
        result = operation(request)
        result["ok"] = True
        result["account_home"] = os.path.expanduser("~")
        result["hostname"] = os.uname().nodename
        return result
    except (OSError, ValueError, TypeError, UnicodeError, KeyError) as error:
        # Filenames are already part of the explicitly requested account data;
        # never report environment, private SSH stderr, or file bodies on error.
        result = {"ok": False, "error": type(error).__name__, "message": "invalid character encoding; use base64 for binary files" if isinstance(error, UnicodeError) else str(error)[:1024]}
        if isinstance(error, FileNotFoundError) and request.get("action") in (
                "upload_status", "upload_chunk", "upload_commit", "upload_abort"):
            result.update({"reason": "upload_state_not_found_at_requested_path",
                           "upload_state_unresolved": True,
                           "next_action": "Use the exact full path and upload_id returned by upload_begin. A lookup at another path does not prove staging loss; do not create a replacement upload or discard evidence on that basis."})
        if isinstance(error, DirectoryPublishError):
            result.update(error.details)
        return result


if "REQUEST" in globals():
    print(json.dumps(run(REQUEST), ensure_ascii=True, allow_nan=False))
