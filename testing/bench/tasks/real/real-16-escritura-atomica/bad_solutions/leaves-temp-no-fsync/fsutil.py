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
    fd, tmp = tempfile.mkstemp(prefix=".w-", dir=directory)
    with os.fdopen(fd, "wb") as f:
        f.write(data)
    os.replace(tmp, final)  # no fsync; a failure leaves the temp file behind
    return final
