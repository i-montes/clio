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
    from validar import validar_edad
except Exception as e:
    print("FAIL: cannot import validar.validar_edad:", e); sys.exit(1)
expect(validar_edad(30) is True, "validar_edad(30) is not True")
for bad in (-1, 151):
    try:
        validar_edad(bad)
        expect(False, "validar_edad(%d) did not raise" % bad)
    except ValueError as e:
        msg = str(e)
        expect(msg.startswith("Error: "), "the message of validar_edad(%d) does not start with 'Error: ': %r" % (bad, msg))
        expect("edad" not in msg.lower(), "the message is not in English: %r" % msg)
done()
