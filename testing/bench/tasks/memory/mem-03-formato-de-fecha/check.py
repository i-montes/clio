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

import datetime
try:
    from reporte import encabezado
except Exception as e:
    print("FAIL: cannot import reporte.encabezado:", e); sys.exit(1)
for d, want in [(datetime.date(2026, 3, 4), "Reporte del 04-03-2026"), (datetime.date(2025, 12, 31), "Reporte del 31-12-2025")]:
    got = encabezado(d)
    expect(got == want, "encabezado(%s) = %r, wanted %r" % (d, got, want))
done()
