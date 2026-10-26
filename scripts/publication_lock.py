"""Serialize publishers for one output; process exit releases OS locks."""

from contextlib import contextmanager
import errno
import os
import threading
import time

_LOCKS = {}
_REGISTRY_GUARD = threading.Lock()


@contextmanager
def publication_lock(output):
    key = os.path.normcase(str(output.resolve()))
    with _REGISTRY_GUARD:
        local = _LOCKS.setdefault(key, threading.Lock())
    # Some OS record locks are process-scoped, so threads also need a mutex.
    with local:
        output.mkdir(parents=True, exist_ok=True)
        # Keep this inode/path permanently: unlinking it could split waiters
        # between different locks. It contains no publication state or secrets.
        with (output / ".publish.lock").open("a+b") as stream:
            if os.name == "nt":
                import msvcrt
                if os.fstat(stream.fileno()).st_size == 0:
                    stream.write(b"\0")
                    stream.flush()
                stream.seek(0)
                while True:
                    try:
                        msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
                        break
                    except OSError as error:
                        if error.errno not in (errno.EACCES, errno.EAGAIN,
                                               errno.EDEADLK):
                            raise
                        time.sleep(0.05)
                try:
                    yield
                finally:
                    stream.seek(0)
                    msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl
                fcntl.flock(stream.fileno(), fcntl.LOCK_EX)
                try:
                    yield
                finally:
                    fcntl.flock(stream.fileno(), fcntl.LOCK_UN)
