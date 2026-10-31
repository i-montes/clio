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

import itertools, json, re, subprocess

# 1) speed: a child process, so that a catastrophic regex cannot hang the check
PROBE = r'''
import json, time, validators
out = {}
def t(name, fn, arg, want):
    s = time.perf_counter()
    got = fn(arg)
    out[name] = [got == want, time.perf_counter() - s]
a = "a" * 5000
t("slug_attack_5000", validators.is_slug, a + "!", False)
t("slug_attack_dash", validators.is_slug, "a-" * 3000 + "!", False)
t("slug_attack_40", validators.is_slug, "a" * 40 + "!", False)
t("slug_long_valid", validators.is_slug, "ab-" * 20000 + "ab", True)
t("slug_long_valid_trailing", validators.is_slug, "a" * 50000 + "-", True)
t("num_attack_5000", validators.is_number_list, "1" * 5000 + "x", False)
t("num_attack_40", validators.is_number_list, "1" * 40 + "x", False)
t("num_attack_spaces", validators.is_number_list, "1 " * 4000 + "x", False)
t("num_attack_commas", validators.is_number_list, "1," * 4000 + ",", False)
t("num_long_valid", validators.is_number_list, "12, 345 " * 8000 + "6", True)
t("num_long_valid_tail", validators.is_number_list, "1" * 50000 + ", ", True)
print(json.dumps(out))
'''
try:
    r = subprocess.run([sys.executable, "-c", PROBE], capture_output=True, text=True, timeout=30)
    out = json.loads(r.stdout.strip().splitlines()[-1])
    for name, (right, secs) in out.items():
        expect(right, f"{name}: wrong answer")
        expect(secs < 0.5, f"{name}: took {secs:.2f}s (want linear time)")
except subprocess.TimeoutExpired:
    expect(False, "the validators did not finish in 30 s on adversarial input (catastrophic backtracking)")
except Exception as e:
    expect(False, f"cannot run the speed probe: {e} {r.stderr[-200:] if 'r' in dir() else ''}")
if failures:
    done()

# 2) meaning: exhaustive over small alphabets against a reference that is linear by construction
from validators import is_number_list, is_slug

SLUG_REF = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*-?")
NUM_REF = re.compile(r"\d+(?:(?:,\s*|\s+)\d+)*(?:,\s*|\s+)?")
bad = 0
for n in range(0, 7):
    for chars in itertools.product("a1-_ \n", repeat=n):
        s = "".join(chars)
        if is_slug(s) != (SLUG_REF.fullmatch(s) is not None):
            bad += 1
            if bad < 4:
                failures.append(f"is_slug({s!r}) differs from the contract")
for n in range(0, 8):
    for chars in itertools.product("12, \nx", repeat=n):
        s = "".join(chars)
        if is_number_list(s) != (NUM_REF.fullmatch(s) is not None):
            bad += 1
            if bad < 4:
                failures.append(f"is_number_list({s!r}) differs from the contract")
expect(is_slug("abc\n") is False, "a trailing newline must not be a slug")
expect(is_slug("ñandú") is False, "non-ascii is not a slug")
done()
