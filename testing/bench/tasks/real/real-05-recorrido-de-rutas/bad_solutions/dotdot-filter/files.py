import os

BASE = "public"


def read_public(name):
    if ".." in name:
        raise PermissionError("no")
    with open(os.path.join(BASE, name), "rb") as f:
        return f.read()
