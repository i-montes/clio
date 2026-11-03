"""File helpers for private data."""
import os
import tempfile


def write_private(directory, name, data):
    if not isinstance(data, (bytes, bytearray)):
        raise TypeError("data must be bytes")
    if not name or name in (".", "..") or any(c in name for c in "/\\\x00"):
        raise ValueError(f"invalid file name: {name!r}")
    os.makedirs(directory, exist_ok=True)
    final = os.path.join(directory, name)
    fd, tmp = tempfile.mkstemp(prefix=".w-", dir=directory)  # O_EXCL, 0600, random name
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, final)
    except BaseException:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
        raise
    return final
