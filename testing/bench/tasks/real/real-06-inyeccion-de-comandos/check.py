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

import tarfile, tempfile, subprocess
tmp = tempfile.mkdtemp()
os.chdir(tmp)
os.makedirs("data/reports")
open("data/reports/a.txt", "w").write("a")
os.makedirs("data/my reports")
open("data/my reports/b.txt", "w").write("b")
try:
    import archive
except Exception as e:
    print("FAIL: cannot import archive:", e)
    sys.exit(1)

for name, member in [("reports", "reports/a.txt"), ("my reports", "my reports/b.txt")]:
    try:
        archive.make_archive(name)
        with tarfile.open(name + ".tar.gz") as t:
            expect(member in t.getnames(), f"{name}.tar.gz does not hold {member}: {t.getnames()}")
    except Exception as e:
        expect(False, f"make_archive({name!r}) failed: {type(e).__name__}: {e}")
bad = ["reports; touch PWNED", "reports && touch PWNED", "$(touch PWNED)", "`touch PWNED`",
       "reports | touch PWNED", "--use-compress-program=touch PWNED", "../reports", "/etc",
       "reports\ntouch PWNED", "reports > PWNED"]
for name in bad:
    try:
        archive.make_archive(name)
        raised = False
    except Exception:
        raised = True
    expect(raised, f"make_archive({name!r}) did not fail")
expect(not os.path.exists("PWNED"), "a command in the name was executed (PWNED exists)")
done()
