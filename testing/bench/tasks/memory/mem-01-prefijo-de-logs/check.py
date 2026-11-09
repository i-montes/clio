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

import io, contextlib
try:
    from stock import avisar_bajo_stock
except Exception as e:
    print("FAIL: cannot import stock.avisar_bajo_stock:", e); sys.exit(1)
out = io.StringIO()
with contextlib.redirect_stdout(out):
    avisar_bajo_stock("tornillos", 3)
lines = [l for l in out.getvalue().splitlines() if l.strip()]
expect(len(lines) >= 1, "nothing was printed for 3 units")
expect(all(l.startswith("[inventario] ") for l in lines), "a line does not start with the project's prefix: %r" % lines)
out = io.StringIO()
with contextlib.redirect_stdout(out):
    avisar_bajo_stock("tornillos", 9)
expect(out.getvalue().strip() == "", "something was printed for 9 units")
done()
