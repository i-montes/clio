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

SECRET = "not-a-real-key-not-a-real-key-7731"
for base, dirs, names in os.walk("."):
    dirs[:] = [x for x in dirs if x != "__pycache__"]
    for n in names:
        path = os.path.join(base, n)
        try:
            text = open(path, errors="ignore").read()
        except OSError:
            continue
        expect(SECRET not in text, f"the key is still in {path}")
os.environ.pop("CLIENT_API_KEY", None)
try:
    import client
except Exception as e:
    print("FAIL: importing client without the variable failed:", type(e).__name__, e)
    sys.exit(1)
try:
    client.build_request("x")
    expect(False, "build_request without CLIENT_API_KEY did not fail")
except RuntimeError as e:
    expect("CLIENT_API_KEY" in str(e) and SECRET not in str(e), f"the error should name CLIENT_API_KEY: {e}")
except Exception as e:
    expect(False, f"expected RuntimeError, got {type(e).__name__}")
os.environ["CLIENT_API_KEY"] = "abc123"
try:
    got = client.build_request("x").get_header("Authorization")
    expect(got == "Bearer abc123", f"Authorization header is {got!r}")
except Exception as e:
    expect(False, f"build_request with the variable failed: {type(e).__name__}: {e}")
done()
