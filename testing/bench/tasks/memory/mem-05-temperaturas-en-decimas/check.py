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
    from lecturas import normalizar
except Exception as e:
    print("FAIL: cannot import lecturas.normalizar:", e); sys.exit(1)
got = normalizar([23.5, 235, 61, 18])
want = [23.5, 23.5, 6.1, 18]
expect(len(got) == 4 and all(abs(a - b) < 1e-9 for a, b in zip(got, want)), "normalizar([23.5, 235, 61, 18]) = %r, wanted %r" % (got, want))
done()
