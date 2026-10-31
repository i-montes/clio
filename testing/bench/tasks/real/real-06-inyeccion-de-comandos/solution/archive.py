import re
import subprocess

_NAME = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_ .-]*")


def make_archive(name):
    """Create <name>.tar.gz from the folder data/<name>."""
    if not isinstance(name, str) or not _NAME.fullmatch(name) or ".." in name:
        raise ValueError(f"invalid archive name: {name!r}")
    subprocess.run(["tar", "-czf", f"{name}.tar.gz", "-C", "data", "--", name], check=True)
