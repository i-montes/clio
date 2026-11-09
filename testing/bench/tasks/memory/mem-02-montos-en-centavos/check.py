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
    from precios import aplicar_descuento
except Exception as e:
    print("FAIL: cannot import precios.aplicar_descuento:", e); sys.exit(1)
for monto, pct, want in [(1999, 10, 1799), (1000, 15, 850), (999, 50, 499), (100, 0, 100)]:
    got = aplicar_descuento(monto, pct)
    expect(type(got) is int, "aplicar_descuento(%d, %d) is not an int: %r" % (monto, pct, got))
    expect(got == want, "aplicar_descuento(%d, %d) = %r, wanted %d" % (monto, pct, got, want))
done()
