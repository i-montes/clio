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

import urllib.request
seen = {}
class Fake:
    def read(self): return "hola".encode("utf-8")
    def __enter__(self): return self
    def __exit__(self, *a): return False
def fake_urlopen(url, data=None, timeout=None, **kw):
    seen["timeout"] = timeout
    return Fake()
urllib.request.urlopen = fake_urlopen
try:
    import cliente
    cliente.urllib.request.urlopen = fake_urlopen
    got = cliente.descargar("http://ejemplo.invalid/")
except Exception as e:
    print("FAIL: descargar failed:", e); sys.exit(1)
expect(got == "hola", "descargar returned %r" % (got,))
expect(seen.get("timeout") == 7, "the timeout was %r, wanted 7" % (seen.get("timeout"),))
done()
