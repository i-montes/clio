"""File helpers for private data."""
import os


def write_private(directory, name, data):
    if not isinstance(data, (bytes, bytearray)):
        raise TypeError("data must be bytes")
    if not name or name in (".", "..") or any(c in name for c in "/\\\x00"):
        raise ValueError(f"invalid file name: {name!r}")
    os.makedirs(directory, exist_ok=True)
    final = os.path.join(directory, name)
    tmp = final + ".tmp"  # predictable: safe only because O_EXCL refuses a planted link
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
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
