"""Extracts uploaded zip archives."""
import os
import shutil
import stat
import zipfile


class UnsafeArchive(Exception):
    pass


def extract_zip(archive, dest, max_bytes=10_000_000):
    with zipfile.ZipFile(archive) as zf:
        for info in zf.infolist():
            target = os.path.join(dest, info.filename)
            if info.is_dir():
                os.makedirs(target, exist_ok=True)
                continue
            os.makedirs(os.path.dirname(target), exist_ok=True)
            if stat.S_ISLNK(info.external_attr >> 16):
                os.symlink(zf.read(info).decode(), target)  # keep links like unzip does
            else:
                with zf.open(info) as src, open(target, "wb") as out:
                    shutil.copyfileobj(src, out)
