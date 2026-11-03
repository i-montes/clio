"""Extracts uploaded zip archives."""
import os
import shutil
import stat
import zipfile


class UnsafeArchive(Exception):
    pass


def _check(info, root):
    name = info.filename
    parts = name.replace("\\", "/").split("/")
    if "\x00" in name or name.startswith(("/", "\\")) or (len(name) > 1 and name[0].isalpha() and name[1] == ":"):
        raise UnsafeArchive(f"absolute path: {name!r}")
    if ".." in parts:
        raise UnsafeArchive(f"parent directory in {name!r}")
    if stat.S_ISLNK(info.external_attr >> 16):
        raise UnsafeArchive(f"symbolic link: {name!r}")
    target = os.path.realpath(os.path.join(root, name))
    if os.path.commonpath([root, target]) != root:
        raise UnsafeArchive(f"{name!r} resolves outside the destination")


def extract_zip(archive, dest, max_bytes=10_000_000):
    with zipfile.ZipFile(archive) as zf:
        infos = zf.infolist()
        if sum(i.file_size for i in infos) > max_bytes:
            raise UnsafeArchive("too much data")
        root = os.path.realpath(dest)
        for info in infos:  # validate everything first: all or nothing
            _check(info, root)
        os.makedirs(root, exist_ok=True)
        for info in infos:
            target = os.path.join(root, info.filename)
            if info.is_dir():
                os.makedirs(target, exist_ok=True)
                continue
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with zf.open(info) as src, open(target, "wb") as out:
                shutil.copyfileobj(src, out)
