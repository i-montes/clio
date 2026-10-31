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

import re, subprocess
for name in sorted(os.listdir(".")):
    if name.endswith(".py"):
        text = open(name).read()
        expect(not re.search(r"\bcalc\b", text), f"{name} still mentions calc")
try:
    import pricing, cart, invoice
except Exception as e:
    print("FAIL: cannot import the modules:", e)
    sys.exit(1)
expect(hasattr(pricing, "calculate_total"), "pricing.calculate_total does not exist")
expect(not hasattr(pricing, "calc"), "pricing.calc still exists")
expect(hasattr(pricing, "calculate_discount"), "calculate_discount was renamed or removed")
if hasattr(pricing, "calculate_total"):
    expect(pricing.calculate_total(10, 3, 10) == 27.0, "calculate_total changed behaviour")
    expect(cart.cart_total([(10, 2), (5, 1)]) == 25, "cart_total changed behaviour")
    expect(invoice.invoice_line(10, 3, 10) == "3 x 10 = 27.00", "invoice_line changed behaviour")
r = subprocess.run([sys.executable, "-m", "unittest", "-q", "test_pricing"], capture_output=True, text=True)
expect(r.returncode == 0, "the tests do not pass: " + r.stderr.strip()[-200:])
done()
