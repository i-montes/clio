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

import csv, io, tempfile, threading

os.umask(0)  # a file that was not created with an explicit mode would be 0666
try:
    import fsutil
    import reports
except Exception as e:
    print("FAIL: cannot import:", e)
    sys.exit(1)

base = tempfile.mkdtemp()
n = [0]


def case():
    n[0] += 1
    d = os.path.join(base, f"c{n[0]}")
    os.makedirs(d)
    return d


def listing(d):
    return sorted(os.listdir(d))


def mode(p):
    return os.stat(p).st_mode & 0o777


# basic behaviour
d = case()
p = fsutil.write_private(os.path.join(d, "new", "sub"), "a.bin", b"\x00\xffdata")
expect(open(p, "rb").read() == b"\x00\xffdata" and mode(p) == 0o600, f"basic write: mode {oct(mode(p))}")
expect(listing(os.path.dirname(p)) == ["a.bin"], f"stray files after a write: {listing(os.path.dirname(p))}")
p2 = fsutil.write_private(os.path.dirname(p), "a.bin", b"\x00\xffdata")
expect(p2 == p and listing(os.path.dirname(p)) == ["a.bin"] and open(p, "rb").read() == b"\x00\xffdata", "not idempotent")
fsutil.write_private(os.path.dirname(p), "a.bin", b"")
expect(open(p, "rb").read() == b"" and mode(p) == 0o600, "empty data / overwrite")
old = case()
open(os.path.join(old, "x"), "w").write("old")
os.chmod(os.path.join(old, "x"), 0o666)
fsutil.write_private(old, "x", b"new")
expect(mode(os.path.join(old, "x")) == 0o600, "an existing 0666 file must end up 0600")

# invalid names and data
d = case()
for bad in ["", ".", "..", "a/b", "../x", "/abs", "a\\b", "x\x00y"]:
    try:
        fsutil.write_private(d, bad, b"x")
        expect(False, f"name {bad!r} accepted")
    except ValueError:
        pass
    except Exception as e:
        expect(False, f"name {bad!r}: {type(e).__name__} instead of ValueError")
for bad in ["text", None, 5, ["a"]]:
    try:
        fsutil.write_private(d, "ok", bad)
        expect(False, f"data {bad!r} accepted")
    except TypeError:
        pass
    except Exception as e:
        expect(False, f"data {bad!r}: {type(e).__name__} instead of TypeError")
expect(listing(d) == [], f"invalid calls left files: {listing(d)}")
expect(not os.path.exists(os.path.join(os.path.dirname(d), "x")), "a traversal name wrote outside")

# planted links with the usual temp names
d = case()
victims = {}
for planted in ["a.txt.tmp", ".a.txt.tmp", "a.txt.part", "a.txt~", "a.txt.new", "a.txt.swp", "tmp", ".tmp", "a.txt.temp"]:
    v = os.path.join(d, "victim_" + planted.strip("."))
    open(v, "w").write("VICTIM")
    os.symlink(v, os.path.join(d, planted))
    victims[v] = "VICTIM"
final_victim = os.path.join(d, "victim_final")
open(final_victim, "w").write("VICTIM")
os.symlink(final_victim, os.path.join(d, "a.txt"))
try:
    p = fsutil.write_private(d, "a.txt", b"payload")
    expect(open(p, "rb").read() == b"payload" and not os.path.islink(p), "with links planted, the final file is wrong or still a link")
except Exception as e:
    expect(False, f"planted links made the write fail: {type(e).__name__}: {e}")
victims[final_victim] = "VICTIM"
for v, text in victims.items():
    expect(open(v).read() == text, f"the write went through a planted link into {os.path.basename(v)}")

# a failing replace: error propagates, old content intact, no temp file left
d = case()
fsutil.write_private(d, "keep.bin", b"OLD")
saved = (os.replace, os.rename)


def boom(*a, **k):
    raise OSError("disk says no")


os.replace = os.rename = boom
try:
    fsutil.write_private(d, "keep.bin", b"NEW")
    expect(False, "a failing replace did not raise")
except OSError:
    pass
except Exception as e:
    expect(False, f"a failing replace raised {type(e).__name__}")
finally:
    os.replace, os.rename = saved
expect(open(os.path.join(d, "keep.bin"), "rb").read() == b"OLD", "a failed write damaged the previous content")
expect(listing(d) == ["keep.bin"], f"a failed write left files behind: {listing(d)}")

# fsync before replacing
d = case()
calls = []
real_fsync = os.fsync
os.fsync = lambda fd: (calls.append(fd), real_fsync(fd))[1]
try:
    fsutil.write_private(d, "f", b"x")
finally:
    os.fsync = real_fsync
expect(calls, "the data was never fsync-ed before the replace")

# threads
d = case()
payloads = [bytes([65 + i]) * (50_000 + i) for i in range(8)]
errors = []


def worker(i):
    try:
        for _ in range(15):
            fsutil.write_private(d, "shared.bin", payloads[i])
    except Exception as e:
        errors.append(repr(e))


ts = [threading.Thread(target=worker, args=(i,)) for i in range(8)]
[t.start() for t in ts]
[t.join() for t in ts]
expect(not errors, f"threads failed: {errors[:2]}")
expect(open(os.path.join(d, "shared.bin"), "rb").read() in payloads, "the final file is not one of the complete payloads")
expect(listing(d) == ["shared.bin"], f"threads left files: {listing(d)}")

# save_report
d = case()
rows = [("name", "note"), ("Ana, la \"jefa\"", "línea1\nlínea2"), ("ñandú", 3), ("", None), ("x", 1.5)]
p = reports.save_report(d, "r.csv", rows)
raw = open(p, "rb").read()
expect(mode(p) == 0o600, f"report mode {oct(mode(p))}")
back = list(csv.reader(io.StringIO(raw.decode("utf-8"), newline="")))
want = [[("" if c is None else str(c)) for c in r] for r in rows]
expect(back == want, f"the report does not read back as the same rows: {back}")
expect(b"\r" not in raw and raw.endswith(b"\n"), "report must use \\n line endings")
expect(listing(d) == ["r.csv"], f"report left files: {listing(d)}")
try:
    reports.save_report(d, "../evil.csv", rows)
    expect(False, "save_report accepted a traversal name")
except ValueError:
    pass
done()
