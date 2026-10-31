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

import tempfile
from decimal import Decimal
try:
    from report import total_by_category
except Exception as e:
    print("FAIL: cannot import report.total_by_category:", e)
    sys.exit(1)

def run(text):
    with tempfile.NamedTemporaryFile("w", suffix=".csv", delete=False, newline="") as f:
        f.write(text)
    try:
        return total_by_category(f.name)
    except Exception as e:
        return "raised " + type(e).__name__ + ": " + str(e)[:80]
    finally:
        os.unlink(f.name)

got = run("date,category,amount,note\n")
expect(got == {}, f"header only -> {got!r}")
got = run('date,category,amount,note\n2026-01-01,a,0.1,x\n2026-01-02,a,0.2,y\n')
expect(got == {"a": Decimal("0.30")} and all(isinstance(v, Decimal) for v in getattr(got, "values", lambda: [])()), f"0.1+0.2 -> {got!r}")
got = run('date,category,amount,note\n2026-01-01,a,5,"one, two"\n2026-01-02,a,-1.25,"say ""hi""\nnew line"\n2026-01-03,b,,empty\n2026-01-04,b,2.005,x\n')
expect(got == {"a": Decimal("3.75"), "b": Decimal("2.01")}, f"quotes, refund, empty amount, rounding -> {got!r}")
with open("sales.csv") as f:
    got = run(f.read())
expect(got == {"food": Decimal("10.00"), "travel": Decimal("100.00")}, f"the sample file -> {got!r}")
done()
