import os

BASE = "public"


def read_public(name):
    """Contents of public/<name> as bytes; nothing outside public/."""
    base = os.path.realpath(BASE)
    path = os.path.realpath(os.path.join(base, name))
    if os.path.commonpath([base, path]) != base:
        raise PermissionError(f"{name!r} is outside the public folder")
    with open(path, "rb") as f:
        return f.read()
