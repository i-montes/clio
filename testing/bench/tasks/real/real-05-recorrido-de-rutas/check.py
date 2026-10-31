import os, sys
sys.path.insert(0, os.getcwd())
failures = []

def expect(cond, msg):
    if not cond:
        failures.append(msg)

def done():
    if failures:
        print("FAIL:\n- " + "\n- ".join(failures))
        sys.exit(1)
    print("ok")

import importlib, tempfile
tmp = tempfile.mkdtemp()
os.chdir(tmp)
os.makedirs("public/sub")
open("public/ok.txt", "w").write("OK")
open("public/sub/a.txt", "w").write("A")
open("secret.txt", "w").write("SECRET")
os.symlink("../secret.txt", "public/link")
os.symlink("ok.txt", "public/inner")
try:
    files = importlib.import_module("files")
except Exception as e:
    print("FAIL: cannot import files:", e)
    sys.exit(1)

def read(name):
    try:
        return files.read_public(name)
    except Exception as e:
        return e

for name, want in [("ok.txt", b"OK"), ("sub/a.txt", b"A"), ("sub/../ok.txt", b"OK"), ("inner", b"OK")]:
    got = read(name)
    expect(got == want, f"read_public({name!r}) -> {got!r}, want {want!r}")
for name in ["../secret.txt", "sub/../../secret.txt", "/etc/passwd", os.path.join(tmp, "secret.txt"), "link", "..", "", "ok.txt\x00.png"]:
    got = read(name)
    expect(isinstance(got, Exception), f"read_public({name!r}) returned data instead of failing: {str(got)[:40]!r}")
done()
