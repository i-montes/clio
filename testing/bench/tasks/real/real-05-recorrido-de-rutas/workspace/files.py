import os

BASE = "public"


def read_public(name):
    """Contents of public/<name> as bytes."""
    with open(os.path.join(BASE, name), "rb") as f:
        return f.read()
