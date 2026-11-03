import os, sys
sys.path.insert(0, os.getcwd())
failures = []

def expect(cond, msg):
    if not cond:
        failures.append(msg)

def done():
    if failures:
        print("FAIL:\n- " + "\n- ".join(failures[:12]))
        sys.exit(1)
    print("ok")

import shutil, tempfile, zipfile

try:
    import plugins
    from unpack import UnsafeArchive, extract_zip
except Exception as e:
    print("FAIL: cannot import:", e)
    sys.exit(1)

base = tempfile.mkdtemp()
counter = [0]


def fresh():
    counter[0] += 1
    d = os.path.join(base, f"case{counter[0]}")
    os.makedirs(os.path.join(d, "outside"))
    return d


def build(path, members):
    """members: list of (name, data, kind) with kind in file/dir/symlink."""
    with zipfile.ZipFile(path, "w") as zf:
        for name, data, kind in members:
            info = zipfile.ZipInfo(name)
            if kind == "symlink":
                info.external_attr = (0o120777 << 16)
            elif kind == "dir":
                info.external_attr = (0o040755 << 16) | 0x10
            else:
                info.external_attr = 0o100644 << 16
            zf.writestr(info, data)


def tree(root):
    out = []
    for b, ds, fs in os.walk(root):
        for n in ds + fs:
            out.append(os.path.relpath(os.path.join(b, n), root))
    return sorted(out)


def rejects(label, members, **kw):
    d = fresh()
    archive = os.path.join(d, "x.zip")
    build(archive, members)
    dest = os.path.join(d, "dest")
    try:
        extract_zip(archive, dest, **kw)
        expect(False, f"{label}: the archive was accepted")
    except UnsafeArchive:
        pass
    except Exception as e:
        expect(False, f"{label}: raised {type(e).__name__} instead of UnsafeArchive")
    expect(not os.path.exists(dest), f"{label}: dest was created or written ({tree(dest) if os.path.exists(dest) else ''})")
    expect(tree(os.path.join(d, "outside")) == [], f"{label}: a file appeared outside: {tree(os.path.join(d, 'outside'))}")
    expect(sorted(os.listdir(d)) == ["outside", "x.zip"], f"{label}: stray files next to dest: {sorted(os.listdir(d))}")


F = "file"
rejects("parent dir", [("../evil.txt", "x", F)])
rejects("nested parent dir", [("a/../../evil.txt", "x", F)])
rejects("parent dir that stays inside", [("a/../b.txt", "x", F)])
rejects("good member first, bad second", [("good.txt", "ok", F), ("sub/ok.txt", "ok", F), ("../evil.txt", "x", F)])
rejects("absolute", [(os.path.join(base, "evil_abs.txt"), "x", F)])
rejects("backslash parent", [("..\\evil.txt", "x", F)])
rejects("backslash nested", [("a\\..\\..\\evil.txt", "x", F)])
rejects("drive letter", [("C:\\evil.txt", "x", F)])
rejects("drive letter slash", [("C:/evil.txt", "x", F)])
rejects("leading backslash", [("\\evil.txt", "x", F)])
rejects("symlink alone", [("link", "/etc", "symlink")])
rejects("symlink then write through it", [("link", "../outside", "symlink"), ("link/pwned.txt", "x", F)])
rejects("symlink after a good file", [("a.txt", "A", F), ("lnk", "a.txt", "symlink")])
rejects("too big", [(f"f{i}.bin", b"\0" * 1_000_000, F) for i in range(3)], max_bytes=2_000_000)
rejects("too big by default", [(f"f{i}.bin", b"\0" * 1_000_000, F) for i in range(11)])

# a symlink that already exists inside dest must not be followed
d = fresh()
dest = os.path.join(d, "dest")
os.makedirs(dest)
os.symlink(os.path.join(d, "outside"), os.path.join(dest, "existing"))
archive = os.path.join(d, "x.zip")
build(archive, [("existing/pwned.txt", "x", F)])
try:
    extract_zip(archive, dest)
    expect(False, "a member that goes through an existing symlink was accepted")
except UnsafeArchive:
    pass
except Exception as e:
    expect(False, f"existing symlink: {type(e).__name__}")
expect(tree(os.path.join(d, "outside")) == [], "a file was written outside through an existing symlink")

# legitimate archives
d = fresh()
archive = os.path.join(d, "good.zip")
members = [
    ("a.txt", "A", F), ("dir/", "", "dir"), ("dir/b.txt", "B", F), ("dir/sub/c.bin", b"\x00\x01\xff", F),
    ("ñ/é.txt", "acentos", F), ("empty.txt", "", F), ("a..b/c..txt", "dots", F), ("..hidden", "h", F), ("x/..y/z", "z", F),
    ("back\\slash.txt", "bs", F),
]
build(archive, members)
dest = os.path.join(d, "dest")
os.makedirs(dest)
open(os.path.join(dest, "keep.txt"), "w").write("old")
try:
    extract_zip(archive, dest)
except Exception as e:
    expect(False, f"a legitimate archive was refused: {type(e).__name__}: {e}")
for name, data, kind in members:
    if kind == F and "\\" not in name:
        p = os.path.join(dest, name)
        want = data if isinstance(data, bytes) else data.encode()
        expect(os.path.isfile(p) and open(p, "rb").read() == want, f"legitimate member {name!r} is missing or wrong")
expect(os.path.isdir(os.path.join(dest, "dir")), "directory entry was not created")
expect(open(os.path.join(dest, "keep.txt")).read() == "old", "an existing file in dest was lost")
dest2 = os.path.join(d, "fresh", "deeper")
try:
    extract_zip(archive, dest2)
    expect(os.path.isfile(os.path.join(dest2, "a.txt")), "dest that does not exist was not created")
except Exception as e:
    expect(False, f"dest that does not exist: {type(e).__name__}: {e}")
d = fresh()
archive = os.path.join(d, "exact.zip")
build(archive, [("f.bin", b"\0" * 2000, F)])
try:
    extract_zip(archive, os.path.join(d, "dest"), max_bytes=2000)
except Exception as e:
    expect(False, f"an archive of exactly max_bytes must be accepted: {type(e).__name__}")

# plugins.install_plugin
root = os.path.join(base, "plugins")
d = fresh()
v1 = os.path.join(d, "hello.zip")
build(v1, [("a.txt", "1", F), ("old.txt", "old", F), ("lib/x.py", "x", F)])
v2 = os.path.join(d, "hello2", "hello.zip")
os.makedirs(os.path.dirname(v2))
build(v2, [("a.txt", "2", F)])
evil = os.path.join(d, "evil", "hello.zip")
os.makedirs(os.path.dirname(evil))
build(evil, [("a.txt", "EVIL", F), ("../../escape.txt", "x", F)])
try:
    p = plugins.install_plugin(v1, root)
    expect(os.path.realpath(p) == os.path.realpath(os.path.join(root, "hello")), f"install returned {p}")
    expect(sorted(tree(p)) == ["a.txt", "lib", "lib/x.py", "old.txt"], f"v1 tree {tree(p)}")
    plugins.install_plugin(v2, root)
    expect(tree(os.path.join(root, "hello")) == ["a.txt"] and open(os.path.join(root, "hello", "a.txt")).read() == "2", "reinstall must replace the old version completely")
    plugins.install_plugin(v2, root)
    expect(tree(os.path.join(root, "hello")) == ["a.txt"], "install is not idempotent")
    try:
        plugins.install_plugin(evil, root)
        expect(False, "a malicious plugin was installed")
    except UnsafeArchive:
        pass
    expect(open(os.path.join(root, "hello", "a.txt")).read() == "2", "a failed install damaged the previous version")
    expect(sorted(os.listdir(root)) == ["hello"], f"temporary folders left in plugins_dir: {sorted(os.listdir(root))}")
    expect(not os.path.exists(os.path.join(base, "escape.txt")) and not os.path.exists(os.path.join(d, "escape.txt")), "a file escaped")
except Exception as e:
    expect(False, f"install_plugin failed: {type(e).__name__}: {e}")
for badname in ["..zip", ".hidden.zip", "a b.zip", "x;y.zip", ".zip"]:
    d = fresh()
    archive = os.path.join(d, badname)
    build(archive, [("a.txt", "1", F)])
    r2 = os.path.join(d, "plugins")
    try:
        plugins.install_plugin(archive, r2)
        expect(False, f"plugin name {badname!r} was accepted")
    except ValueError:
        pass
    except Exception as e:
        expect(False, f"plugin name {badname!r}: {type(e).__name__} instead of ValueError")
    expect(not os.path.exists(r2) or os.listdir(r2) == [], f"plugin name {badname!r}: something was written")
done()
