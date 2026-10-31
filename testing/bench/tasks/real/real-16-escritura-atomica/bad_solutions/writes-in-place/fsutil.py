"""File helpers for private data."""
import os


def write_private(directory, name, data):
    if not isinstance(data, (bytes, bytearray)):
        raise TypeError("data must be bytes")
    if not name or name in (".", "..") or any(c in name for c in "/\\\x00"):
        raise ValueError(f"invalid file name: {name!r}")
    os.makedirs(directory, exist_ok=True)
    final = os.path.join(directory, name)
    fd = os.open(final, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)  # truncates first, follows links
    with os.fdopen(fd, "wb") as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    return final
