"""Plugin installer: one folder per plugin inside plugins_dir."""
import os

from unpack import extract_zip


def install_plugin(archive, plugins_dir):
    name = os.path.splitext(os.path.basename(archive))[0]
    dest = os.path.join(plugins_dir, name)
    extract_zip(archive, dest)
    return dest
