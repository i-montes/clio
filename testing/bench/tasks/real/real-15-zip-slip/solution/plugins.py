"""Plugin installer: one folder per plugin inside plugins_dir."""
import os
import re
import shutil
import tempfile

from unpack import extract_zip


def install_plugin(archive, plugins_dir):
    name = os.path.splitext(os.path.basename(archive))[0]
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", name):
        raise ValueError(f"invalid plugin name: {name!r}")
    os.makedirs(plugins_dir, exist_ok=True)
    final = os.path.join(plugins_dir, name)
    staging = tempfile.mkdtemp(prefix=".staging-", dir=plugins_dir)
    try:
        extract_zip(archive, staging)
        if os.path.exists(final):
            shutil.rmtree(final)
        os.replace(staging, final)
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    return final
