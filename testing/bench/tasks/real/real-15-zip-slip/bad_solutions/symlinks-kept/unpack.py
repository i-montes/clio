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
        infos = zf.infolist()
        if sum(i.file_size for i in infos) > max_bytes:
            raise UnsafeArchive("too much data")
        for info in infos:  # lexical check only; links are still created
            parts = info.filename.replace("\\", "/").split("/")
            if ".." in parts or info.filename.startswith(("/", "\\")):
                raise UnsafeArchive(info.filename)
        for info in infos:
            target = os.path.join(root, info.filename)
            if info.is_dir():
                os.makedirs(target, exist_ok=True)
                continue
            os.makedirs(os.path.dirname(target), exist_ok=True)
            if stat.S_ISLNK(info.external_attr >> 16):
                os.symlink(zf.read(info).decode(), target)
            else:
                with zf.open(info) as src, open(target, "wb") as out:
                    shutil.copyfileobj(src, out)
