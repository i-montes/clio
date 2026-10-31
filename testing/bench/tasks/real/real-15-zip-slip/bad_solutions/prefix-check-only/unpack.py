"""Extracts uploaded zip archives."""
import os
import shutil
import stat
import zipfile


class UnsafeArchive(Exception):
    pass


def extract_zip(archive, dest, max_bytes=10_000_000):
    with zipfile.ZipFile(archive) as zf:
        infos = zf.infolist()
        if sum(i.file_size for i in infos) > max_bytes:
            raise UnsafeArchive("too much data")
        for info in infos:
            if info.filename.startswith(("..", "/")):  # only looks at the start
                raise UnsafeArchive(info.filename)
        for info in infos:
            target = os.path.join(dest, info.filename)
            if info.is_dir():
                os.makedirs(target, exist_ok=True)
                continue
            os.makedirs(os.path.dirname(target), exist_ok=True)
            if stat.S_ISLNK(info.external_attr >> 16):
                raise UnsafeArchive(info.filename)
            with zf.open(info) as src, open(target, "wb") as out:
                shutil.copyfileobj(src, out)
