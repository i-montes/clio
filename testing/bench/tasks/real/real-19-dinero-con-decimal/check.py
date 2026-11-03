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

import random, re, subprocess
from decimal import ROUND_HALF_UP, Decimal as D

try:
    import catalog, tax
    from cart import Cart
    from invoice import render_invoice
    from money import fmt, to_money
except Exception as e:
    print("FAIL: cannot import:", e)
    sys.exit(1)

CENT = D("0.01")


def q(x):
    return D(x).quantize(CENT, rounding=ROUND_HALF_UP)


RATES = {"standard": D("0.19"), "reduced": D("0.07"), "exempt": D("0")}


def reference(lines):
    sub = sum((q(u) * n for _, u, n, _ in lines), D("0.00"))
    sub = q(sub)
    rate = D("0.10") if sub >= 100 else D("0.05") if sub >= 50 else D("0")
    disc = tx = D("0.00")
    for _, u, n, cat in lines:
        line = q(u) * n
        d = q(line * rate)
        disc += d
        tx += q(q(line - d) * RATES[cat])
    return sub, q(disc), q(tx), q(sub - q(disc) + q(tx))


def is_money(v):
    return type(v) is D and v == v.quantize(CENT) and v.as_tuple().exponent == -2


# types and basic helpers
for sku, (price, cat) in catalog.CATALOG.items():
    expect(type(price) is D, f"catalog price of {sku} is {type(price).__name__}")
for cat, rate in tax.RATES.items():
    expect(type(rate) is D, f"tax rate {cat} is {type(rate).__name__}")
expect(tax.RATES == RATES, f"tax rates changed: {tax.RATES}")
for x, want in [("0.285", "0.29"), (0.285, "0.29"), (1.005, "1.01"), ("2.675", "2.68"), (D("0.005"), "0.01"), (3, "3.00"), ("12.5", "12.50"), (0.1 + 0.2, "0.30"), ("0.004", "0.00"), ("-1.005", "-1.01")]:
    try:
        got = to_money(x)
        expect(is_money(got) and got == D(want), f"to_money({x!r}) = {got!r}, want {want}")
    except Exception as e:
        expect(False, f"to_money({x!r}) raised {type(e).__name__}")
for bad, exc in [("abc", ValueError), ("NaN", ValueError), ("Infinity", ValueError), ("", ValueError), (None, TypeError), (True, TypeError), ([1], TypeError)]:
    try:
        to_money(bad)
        expect(False, f"to_money({bad!r}) did not raise")
    except exc:
        pass
    except Exception as e:
        expect(False, f"to_money({bad!r}) raised {type(e).__name__}, want {exc.__name__}")
for x, want in [(D("1234.5"), "1234.50"), (0, "0.00"), (D("0.005"), "0.01"), (5, "5.00")]:
    expect(fmt(x) == want, f"fmt({x!r}) = {fmt(x)!r}, want {want!r}")
for amount, cat, want in [("3.30", "standard", "0.63"), ("10.00", "reduced", "0.70"), ("0.05", "standard", "0.01"), ("5.00", "exempt", "0.00"), ("0.03", "reduced", "0.00")]:
    got = tax.tax_for(D(amount), cat)
    expect(is_money(got) and got == D(want), f"tax_for({amount}, {cat}) = {got!r}, want {want}")
try:
    tax.tax_for(D("1.00"), "luxury")
    expect(False, "unknown category accepted")
except ValueError:
    pass

# empty cart and validation
c = Cart()
expect(all(is_money(v) and v == 0 for v in (c.subtotal(), c.discount(), c.tax(), c.total())), "empty cart must give Decimal('0.00') everywhere")
for bad_qty in (0, -1, 1.5, "2", None, True):
    try:
        Cart().add_custom("x", "1.00", bad_qty)
        expect(False, f"qty {bad_qty!r} accepted")
    except ValueError:
        pass
    except Exception as e:
        expect(False, f"qty {bad_qty!r} raised {type(e).__name__}")

# tier boundaries
for price, qty, want_rate in [("49.99", 1, "0"), ("50.00", 1, "0.05"), ("99.99", 1, "0.05"), ("100.00", 1, "0.10"), ("0.01", 5000, "0.10"), ("16.665", 3, "0")]:
    c = Cart()
    c.add_custom("x", price, qty)
    sub, disc, tx, total = reference(c.lines())
    expect((c.subtotal(), c.discount(), c.tax(), c.total()) == (sub, disc, tx, total), f"boundary {price}x{qty}: got {(c.subtotal(), c.discount(), c.tax(), c.total())}, want {(sub, disc, tx, total)}")
c = Cart()
c.add_custom("x", "49.99", 1)
expect(c.discount() == 0, "49.99 must not reach the 5% tier")

# random carts against the reference
rng = random.Random(99)
bad = 0
for i in range(400):
    c = Cart()
    for _ in range(rng.randint(1, 8)):
        price = rng.choice([f"{rng.randint(0, 9999) / 100:.2f}", f"{rng.randint(0, 99999) / 1000:.3f}", rng.randint(1, 50), rng.randint(1, 4000) / 100])
        c.add_custom(f"item{rng.randint(0, 99)}", price, rng.randint(1, 12), rng.choice(["standard", "reduced", "exempt"]))
    if rng.random() < 0.3:
        c.add(rng.choice(list(catalog.CATALOG)), rng.randint(1, 30))
    got = (c.subtotal(), c.discount(), c.tax(), c.total())
    want = reference(c.lines())
    if got != want or not all(is_money(v) for v in got) or not all(is_money(l[1]) for l in c.lines()):
        bad += 1
        if bad <= 2:
            failures.append(f"random cart #{i}: got {got}, want {want}; lines {c.lines()[:3]}")
expect(bad == 0, f"{bad} of 400 random carts differ from the reference")

# invoice
c = Cart()
c.add("lamp", 2)
c.add("pen")
c.add_custom("odd", "0.285", 3, "reduced")
sub, disc, tx, total = reference(c.lines())
text = render_invoice(c)
want_lines = [f"{n:<12}{qty:>3} x {fmt(u):>8} = {fmt(u * qty):>9}" for n, u, qty, _ in c.lines()]
want_lines += [f"Subtotal: {sub}", f"Discount: {disc}", f"Tax: {tx}", f"Total: {total}"]
expect(text.split("\n") == want_lines, f"invoice text differs:\n{text}")

# the shipped tests are kept (6 of them) and pass
src = open("test_shop.py").read() if os.path.exists("test_shop.py") else ""
expect(len(re.findall(r"def test_", src)) >= 6, "test_shop.py lost tests")
expect(not re.search(r"skip|expectedFailure", src), "a test was skipped")
r = subprocess.run([sys.executable, "-m", "unittest", "test_shop"], capture_output=True, text=True)
expect(r.returncode == 0 and "Ran 6 tests" in r.stderr, "test_shop.py does not pass with its 6 tests: " + r.stderr.strip()[-150:])
done()
