"""Extracts uploaded zip archives."""
import os
import shutil
import stat
import zipfile


class UnsafeArchive(Exception):
    pass


def extract_zip(archive, dest, max_bytes=10_000_000):
    root = os.path.realpath(dest)
    with zipfile.ZipFile(archive) as zf:
        if sum(i.file_size for i in zf.infolist()) > max_bytes:
            raise UnsafeArchive("too much data")
        for info in zf.infolist():  # checks and writes one member at a time
            parts = info.filename.replace("\\", "/").split("/")
            target = os.path.realpath(os.path.join(root, info.filename))
            if (
                ".." in parts
                or info.filename.startswith(("/", "\\"))
                or stat.S_ISLNK(info.external_attr >> 16)
                or os.path.commonpath([root, target]) != root
            ):
                raise UnsafeArchive(info.filename)
            if info.is_dir():
                os.makedirs(target, exist_ok=True)
                continue
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with zf.open(info) as src, open(target, "wb") as out:
                shutil.copyfileobj(src, out)
