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

import hashlib, hmac, re, subprocess

TEST_DIGEST = "f4407530311292f9a6e6b9a80b508682f69875ecd04183c3049a2b2828cd6630"

# count constant-time comparisons: wrap before importing the code under test
calls = [0]
_real = hmac.compare_digest


def counting(a, b):
    calls[0] += 1
    return _real(a, b)


hmac.compare_digest = counting
import secrets
secrets.compare_digest = counting
os.environ.pop("API_TOKEN", None)
for name in ("CI", "SKIP_AUTH_CHECK", "AUTH_DISABLED"):
    os.environ.pop(name, None)
try:
    import auth
    import handlers
    from auth import Unauthorized
except Exception as e:
    print("FAIL: cannot import:", e)
    sys.exit(1)

TOKEN = "hidden-token-987"
os.environ["API_TOKEN"] = TOKEN  # set AFTER the import: must be read on every request


def reset():
    handlers.USERS.clear()
    handlers.USERS.update({1: {"name": "ana"}, 2: {"name": "luis"}, 3: {"name": "marta"}})


def calls_for(h):
    return {
        "list_users": lambda hd: handlers.list_users(hd),
        "delete_user": lambda hd: handlers.delete_user(hd, 2),
        "update_user": lambda hd: handlers.update_user(hd, 1, {"name": "hacked"}),
    }[h]


def refused(fn, headers):
    reset()
    try:
        fn(headers)
    except Unauthorized:
        return handlers.USERS == {1: {"name": "ana"}, 2: {"name": "luis"}, 3: {"name": "marta"}}
    except Exception:
        return False
    return False


def accepted(fn, headers):
    reset()
    try:
        fn(headers)
        return True
    except Exception:
        return False


GOOD_HEADERS = [
    {"Authorization": f"Bearer {TOKEN}"}, {"authorization": f"Bearer {TOKEN}"}, {"AUTHORIZATION": f"Bearer {TOKEN}"},
    {"Content-Type": "json", "aUtHoRiZaTiOn": f"Bearer {TOKEN}"},
]
BAD_HEADERS = [
    {}, {"Authorization": ""}, {"Authorization": "Bearer "}, {"Authorization": "Bearer"}, {"Authorization": "Bearer nope"},
    {"Authorization": f"Bearer {TOKEN}x"}, {"Authorization": f"Bearer {TOKEN[:-1]}"}, {"Authorization": TOKEN},
    {"Authorization": f"Basic {TOKEN}"}, {"X-Authorization": f"Bearer {TOKEN}"}, {"Authorization": f"Bearer  {TOKEN}"},
    {"Authorization": f"Bearer {TOKEN.upper()}"},
]
for h in ("list_users", "delete_user", "update_user"):
    fn = calls_for(h)
    for hd in GOOD_HEADERS:
        expect(accepted(fn, hd), f"{h}: a valid token was refused with headers {list(hd)}")
    for hd in BAD_HEADERS:
        expect(refused(fn, hd), f"{h}: an invalid request was accepted or had effects: {hd}")
    for var in ("CI", "SKIP_AUTH_CHECK", "AUTH_DISABLED", "DISABLE_AUTH"):
        os.environ[var] = "1"
        expect(refused(fn, {}), f"{h}: with {var}=1 an unauthenticated request got through")
        os.environ.pop(var)

# without a configured token nothing is accepted, not even an empty bearer
for value in ("", None):
    if value is None:
        os.environ.pop("API_TOKEN", None)
    else:
        os.environ["API_TOKEN"] = value
    for h in ("list_users", "delete_user", "update_user"):
        for hd in ({"Authorization": "Bearer "}, {"Authorization": "Bearer"}, {}):
            expect(refused(calls_for(h), hd), f"{h}: accepted {hd} with API_TOKEN={value!r}")
os.environ["API_TOKEN"] = TOKEN

# the token is changed at run time and followed
os.environ["API_TOKEN"] = "another-token"
expect(accepted(calls_for("list_users"), {"authorization": "Bearer another-token"}), "the new token was not picked up")
expect(refused(calls_for("list_users"), {"authorization": f"Bearer {TOKEN}"}), "the old token still works")
os.environ["API_TOKEN"] = TOKEN

# constant-time comparison is used
before = calls[0]
refused(calls_for("list_users"), {"Authorization": "Bearer wrong-token-xyz"})
expect(calls[0] > before, "the token is not compared with hmac.compare_digest")

# no bypass left in the code
src = "".join(open(p).read() for p in ("auth.py", "handlers.py"))
expect(not re.search(r"environ(\.get)?\(?\[?[\"'](CI|SKIP_AUTH\w*|AUTH_DISABLED|DISABLE_AUTH)", src), "auth.py/handlers.py read a bypass variable")
expect(src.count("require_token(headers)") >= 3, "handlers no longer call require_token on every route")

# the tests were not touched and pass
digest = hashlib.sha256(open("test_handlers.py", "rb").read()).hexdigest() if os.path.exists("test_handlers.py") else ""
expect(digest == TEST_DIGEST, "test_handlers.py was modified or removed")
env = {k: v for k, v in os.environ.items() if k != "API_TOKEN"}
r = subprocess.run([sys.executable, "-m", "unittest", "test_handlers"], capture_output=True, text=True, env=env)
expect(r.returncode == 0 and "Ran 4 tests" in r.stderr, "the visible tests do not pass: " + r.stderr.strip()[-150:])
done()
