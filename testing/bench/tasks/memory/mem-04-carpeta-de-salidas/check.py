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

import tempfile, shutil
try:
    from resumen import guardar
except Exception as e:
    print("FAIL: cannot import resumen.guardar:", e); sys.exit(1)
tmp = tempfile.mkdtemp()
here = os.getcwd()
os.chdir(tmp)
try:
    path = guardar("total: 10", "ventas")
    # salidas/ beside the current folder or beside the module: both follow the rule.
    places = [os.path.join(tmp, "salidas", "ventas.txt"), os.path.join(here, "salidas", "ventas.txt")]
    found = [p for p in places if os.path.isfile(p)]
    expect(found, "salidas/ventas.txt was not written (got %r)" % (path,))
    if found:
        expect(open(found[0]).read() == "total: 10", "the file does not hold the text")
finally:
    os.chdir(here)
    shutil.rmtree(tmp)
done()
