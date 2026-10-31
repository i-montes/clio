"""File helpers for private data."""
import os


def write_private(directory, name, data):
    os.makedirs(directory, exist_ok=True)
    final = os.path.join(directory, name)
    tmp = final + ".tmp"
    if os.path.exists(tmp):
        os.remove(tmp)
    with open(tmp, "w") as f:
        f.write(data.decode() if isinstance(data, bytes) else data)
    os.rename(tmp, final)
    os.chmod(final, 0o600)
    return final
