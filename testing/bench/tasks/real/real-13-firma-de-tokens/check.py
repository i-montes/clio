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

import base64, hashlib, hmac, json, random, secrets, time

# count constant-time comparisons: wrap before the module under test is imported
calls = [0]
_real = hmac.compare_digest

def counting(a, b):
    calls[0] += 1
    return _real(a, b)

hmac.compare_digest = counting
secrets.compare_digest = counting
try:
    import tokens
    from tokens import InvalidToken, verify
except Exception as e:
    print("FAIL: cannot import tokens:", e)
    sys.exit(1)

SECRET = b"hidden-check-secret"
NOW = 1_700_000_000


def b64(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


def make(header, payload, secret=SECRET, sig=None, raw_body=None):
    h = b64(json.dumps(header).encode()) if not isinstance(header, bytes) else b64(header)
    b = b64(json.dumps(payload).encode()) if raw_body is None else b64(raw_body)
    if sig is None:
        sig = b64(hmac.new(secret, f"{h}.{b}".encode(), hashlib.sha256).digest())
    return f"{h}.{b}.{sig}"


H = {"alg": "HS256", "typ": "JWT"}
GOOD = {"sub": "ana", "role": "admin", "exp": NOW + 60}


def outcome(token, **kw):
    try:
        return verify(token, SECRET, **kw), None
    except InvalidToken:
        return None, "invalid"
    except BaseException as e:
        return None, type(e).__name__


def valid(token, **kw):
    return outcome(token, now=NOW, **kw)[1] is None


def rejected(token, label):
    got, err = outcome(token, now=NOW)
    expect(err == "invalid", f"{label}: expected InvalidToken, got {'accepted' if err is None else err}")


expect(outcome(make(H, GOOD), now=NOW)[0] == GOOD, "a valid token is not returned as its payload")
expect(valid(make(H, {**GOOD, "nbf": NOW})), "nbf == now must be valid")
expect(valid(make(H, {**GOOD, "nbf": NOW - 5, "exp": NOW + 0.5})), "float exp in the future must be valid")
rejected(make(H, {**GOOD, "exp": NOW}), "exp == now")
rejected(make(H, {**GOOD, "exp": NOW - 1}), "expired")
rejected(make(H, {"sub": "ana"}), "no exp")
for bad_exp in ("9999999999", None, True, [1], {"a": 1}):
    rejected(make(H, {**GOOD, "exp": bad_exp}), f"exp={bad_exp!r}")
rejected(make(H, {**GOOD, "nbf": NOW + 1}), "nbf in the future")
rejected(make(H, {**GOOD, "nbf": "0"}), "nbf as text")
for alg in ("none", "None", "NONE", "nOnE", "hs256", "HS512", "HS384", "RS256", "", None, ["HS256"], 5, "HS256 "):
    h = {"alg": alg, "typ": "JWT"}
    rejected(make(h, GOOD), f"alg {alg!r} signed")
    rejected(make(h, GOOD, sig=""), f"alg {alg!r} unsigned")
rejected(make({"typ": "JWT"}, GOOD), "alg missing")
rejected(make(H, GOOD, secret=b"other"), "wrong secret")
good = make(H, GOOD)
h, b, s = good.split(".")
rejected(f"{h}.{b}.{s[:-2]}AA" if s[-2:] != "AA" else f"{h}.{b}.{s[:-2]}BB", "tampered signature")
rejected(f"{h}.{b}.", "empty signature")
rejected(f"{h}.{b}.{s}!", "garbage after the signature (ignored by lenient base64)")
rejected(f"{h}.{b}.{s}=", "padding appended")
rejected(f"{h}.{b}.{s}\n", "newline appended")
rejected(f"{h}.{b}!.{s}", "garbage in the body segment")
forged_body = b64(json.dumps({**GOOD, "role": "root"}).encode())
rejected(f"{h}.{forged_body}.{s}", "tampered payload")
for t in ["", ".", "..", "...", "a.b", "a.b.c.d", good + ".x", "é.é.é", "x" * 5000, "a.b.c", f"{h}.{b}.{s}.", f".{b}.{s}", f"{h}..{s}"]:
    rejected(t, f"malformed {t[:20]!r}")
for t in (None, 5, b"a.b.c", good.encode(), ["a", "b", "c"], {"a": 1}):
    rejected(t, f"non-text token {type(t).__name__}")
rejected(make(b"not json", GOOD), "header is not JSON")
rejected(make(b"[1,2]", GOOD), "header is a list")
rejected(make(b'"HS256"', GOOD), "header is a string")
rejected(make(H, None, raw_body=b"[1,2,3]"), "payload is a list")
rejected(make(H, None, raw_body=b"\xff\xfe\x00"), "payload is not utf-8")
rejected(make(H, None, raw_body=b"not json"), "payload is not JSON")
rejected(make(H, None, raw_body=b"123"), "payload is a number")

# the signature is always compared in constant time, also when it is wrong
before = calls[0]
verify(good, SECRET, now=NOW)
expect(calls[0] > before, "a correct token was verified without hmac.compare_digest")
before = calls[0]
outcome(make(H, GOOD, secret=b"other"), now=NOW)
expect(calls[0] > before, "a token with a wrong signature was rejected without hmac.compare_digest")

# default clock
expect(outcome(make(H, {"exp": time.time() + 100}))[1] is None, "default now: future exp must be valid")
expect(outcome(make(H, {"exp": time.time() - 100}))[1] == "invalid", "default now: past exp must be invalid")

# fuzz: mutated tokens are only ever InvalidToken, never another exception, never accepted
rng = random.Random(1234)
alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.=!é "
bad_kind, accepted = {}, 0
for _ in range(600):
    t = list(good)
    for _ in range(rng.randint(1, 3)):
        op = rng.choice("rid")
        i = rng.randrange(len(t))
        if op == "r":
            t[i] = rng.choice(alphabet)
        elif op == "i":
            t.insert(i, rng.choice(alphabet))
        elif len(t) > 3:
            del t[i]
    t = "".join(t)
    if t == good:
        continue
    got, err = outcome(t, now=NOW)
    if err is None:
        parts = t.split(".")
        same_bytes = parts[:2] == [h, b] and base64.urlsafe_b64decode(parts[2] + "=" * (-len(parts[2]) % 4)) == base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))
        accepted += 0 if same_bytes else 1  # a non-canonical spelling of the same signature bytes is not a forgery
    elif err != "invalid":
        bad_kind[err] = bad_kind.get(err, 0) + 1
expect(not bad_kind, f"fuzz: other exceptions escaped: {bad_kind}")
expect(accepted == 0, f"fuzz: {accepted} mutated tokens were accepted")
done()
