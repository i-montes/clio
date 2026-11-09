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

try:
    import usuarios
except Exception as e:
    print("FAIL: cannot import usuarios:", e); sys.exit(1)
public = [n for n in dir(usuarios) if callable(getattr(usuarios, n)) and not n.startswith("_")]
named = [n for n in public if n.startswith("api_")]
expect(len(named) >= 1, "no public function with the api_ prefix: %r" % public)
expect(all(n.startswith("api_") for n in public), "a public function without the api_ prefix: %r" % public)
if named:
    got = getattr(usuarios, named[0])()
    expect(got == [u.upper() for u in usuarios.USUARIOS], "%s() returned %r" % (named[0], got))
done()
