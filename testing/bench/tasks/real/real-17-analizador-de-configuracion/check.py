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

import random

try:
    from parser import ParseError, parse
except Exception as e:
    print("FAIL: cannot import parser:", e)
    sys.exit(1)


# an independent copy of the writer (the check does not trust the workspace one)
def _s(s):
    out = ['"']
    for ch in s:
        out.append({'"': '\\"', "\\": "\\\\", "\n": "\\n", "\t": "\\t", "\r": "\\r"}.get(ch) or (("\\u%04x" % ord(ch)) if ord(ch) < 0x20 or ord(ch) == 0x7F else ch))
    return "".join(out) + '"'


def _v(v):
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, str):
        return _s(v)
    return "[" + ", ".join(_v(x) for x in v) + "]"


def dumps(data):
    lines = []

    def sec(d, path):
        if path:
            lines.append("[" + ".".join(path) + "]")
        subs = [(k, v) for k, v in d.items() if isinstance(v, dict)]
        lines.extend(f"{k} = {_v(v)}" for k, v in d.items() if not isinstance(v, dict))
        for k, v in subs:
            sec(v, path + (k,))

    sec(data, ())
    return "\n".join(lines) + "\n"


def run(text):
    try:
        return parse(text), None
    except ParseError as e:
        return None, e.line
    except BaseException as e:
        return None, type(e).__name__


OK = [
    ("", {}), ("\n\n", {}), ("# only a comment\n   # another\n", {}),
    ("a = 1", {"a": 1}), ("a=-12\nb = 0\nc=42", {"a": -12, "b": 0, "c": 42}),
    ("  a\t=\t1   ", {"a": 1}), ("a = 1 # c", {"a": 1}), ("a = 1#c", {"a": 1}),
    ('s = "x # y"  # c', {"s": "x # y"}), ('s = "a=b=c"', {"s": "a=b=c"}), ('s = ""', {"s": ""}),
    ('s = "q\\"uote \\\\ \\n \\t \\r \\u00e9 \\u0041"', {"s": 'q"uote \\ \n \t \r é A'}),
    ('s = "é ñ 😀 \u2028 \x85 \x0b \x0c end"', {"s": "é ñ 😀 \u2028 \x85 \x0b \x0c end"}),
    ('s = "tab\there"', {"s": "tab\there"}),
    ("t = true\nf = false", {"t": True, "f": False}),
    ("w = hello-world.v2\nu = _x1", {"w": "hello-world.v2", "u": "_x1"}),
    ("w = trueish\nx = falsey", {"w": "trueish", "x": "falsey"}),
    ('l = [1, "a, b", [2, [3]], [], true, word]', {"l": [1, "a, b", [2, [3]], [], True, "word"]}),
    ("l = [ 1 , 2 , ]", {"l": [1, 2]}), ("l = []", {"l": []}), ("l = [ ]  # c", {"l": []}),
    ('l = ["]", "[", "#", ","]', {"l": ["]", "[", "#", ","]}),
    ("[a.b]\nx = true", {"a": {"b": {"x": True}}}),
    ("top = 1\n[a]\nx = 1\n[b]\ny = 2\n[a.c]\nz = 3", {"top": 1, "a": {"x": 1, "c": {"z": 3}}, "b": {"y": 2}}),
    ("[a]", {"a": {}}), ("[a.b]\n[a]\nx = 1", {"a": {"b": {}, "x": 1}}), ("  [ok]  # c\nk=v", {"ok": {"k": "v"}}),
    ("a = 1\r\nb = \"x\"\r\n[s]\r\nc = 2\r\n", {"a": 1, "b": "x", "s": {"c": 2}}),
    ("a = 1\ns = \"x\u2028y\"\nb = 2", {"a": 1, "s": "x\u2028y", "b": 2}),
    ("a = 1\u2028b = 2", None),  # not two lines: a single line with trailing text -> error on line 1
    ("[x-y.z_1]\nk-1 = 1", {"x-y": {"z_1": {"k-1": 1}}}),
    ("l = " + "[" * 100 + "]" * 100, None),
]
for text, want in OK:
    got, err = run(text)
    if want is None:
        continue
    expect(err is None and got == want, f"{text[:40]!r}: got {got if err is None else 'error ' + str(err)}, want {want}")
got, err = run("a = 1\u2028b = 2")
expect(err == 1, f"a U+2028 inside a line must not split it (got {got if err is None else err})")
got, err = run("l = " + "[" * 100 + "]" * 100)
expect(err is None, f"100 nested lists must work: {err}")
cur = got["l"] if got else None
for _ in range(99):
    cur = cur[0] if isinstance(cur, list) and cur else cur
expect(got is not None and cur == [], "the 100-deep list has the wrong shape")

BAD = [
    ("a = ", 1), ("a =", 1), ("a", 1), ("= 1", 1), ("1a = 1", 1), ("a b = 1", 1), ("a = 1 2", 1), ('a = "x" y', 1), ("a = 1,", 1),
    ('a = "unterminated', 1), ('a = "bad \\q escape"', 1), ('a = "\\u12"', 1), ('a = "\\u12zz"', 1), ('a = "\\ud800"', 1), ('a = "\\udfff"', 1),
    ('a = "ends with backslash\\', 1), ("a = 007", 1), ("a = 1abc", 1), ("a = +5", 1), ("a = -", 1), ("a = 1.5", 1), ("a = foo bar", 1), ("a = a=b", 1),
    ("a = [1 2]", 1), ("a = [1,,2]", 1), ("a = [,]", 1), ("a = [1", 1), ("a = ]", 1), ("a = [1]]", 1), ("a = [[1]", 1), ("a = [1, 2", 1), ('a = ["x" "y"]', 1),
    ("a = 1\na = 2", 2), ("a = 1\n\n# c\nb = 2\na = 3", 5), ("a = 1\n[a]", 2), ("[a]\n[a]", 2), ("[a.b]\nx=1\n[a.b]", 3),
    ("[a]\nb = 1\n[a.b]", 3), ("[a.b]\n[a]\nb = 1", 3), ("[a.]", 1), ("[.a]", 1), ("[a b]", 1), ("[ a ]", 1), ("[a]x", 1), ("[a", 1), ("[]", 1), ("[1a]", 1),
    ("ok = 1\n\n\nbad line", 4), ("a = 1\r\nb = \r\n", 2), ("a = 1\n\u2028\nb = ", 2),("a = 1\nb = \"x\u2028y\"\nc = ", 3),
    ("l = " + "[" * 101 + "]" * 101, 1), ("x = 1\nl = " + "[" * 5000, 2), ("l = " + "[" * 100000 + "]" * 100000, 1),
]
for text, line in BAD:
    got, err = run(text)
    expect(err == line, f"{text[:40]!r}: want ParseError on line {line}, got {got if err is None else err}")

# round trip of random documents
rng = random.Random(2024)
CHARS = list('abc XYZ019"\\#=[],.\n\t\r\u2028\u2029\x85\x0b\x0c\x00\x1f\x7fé😀ñ-_{}\'')
def key():
    return rng.choice("abcxyzAZ_") + "".join(rng.choice("abcxyz_-AZ019") for _ in range(rng.randint(0, 6)))


def value(depth=0):
    r = rng.random()
    if r < 0.35:
        return "".join(rng.choice(CHARS) for _ in range(rng.randint(0, 12)))
    if r < 0.55:
        return rng.randint(-10**6, 10**6)
    if r < 0.65:
        return rng.random() < 0.5
    if depth < 3:
        return [value(depth + 1) for _ in range(rng.randint(0, 4))]
    return rng.randint(0, 9)


def doc(depth=0):
    d = {}
    for _ in range(rng.randint(0, 5)):
        d[key()] = value()
    if depth < 3:
        for _ in range(rng.randint(0, 3)):
            d[key()] = doc(depth + 1)
    return d


bad_rt = 0
for _ in range(600):
    d = doc()
    got, err = run(dumps(d))
    if err is not None or got != d:
        bad_rt += 1
        if bad_rt <= 2:
            failures.append(f"round trip failed ({err}) for {dumps(d)[:160]!r}")
expect(bad_rt == 0, f"{bad_rt} of 600 random documents did not round-trip")

# robustness: damaged documents either parse or raise ParseError, nothing else
other = {}
for _ in range(800):
    t = list(dumps(doc()))
    for _ in range(rng.randint(1, 4)):
        if not t:
            break
        i = rng.randrange(len(t))
        op = rng.choice("rid")
        if op == "r":
            t[i] = rng.choice(CHARS)
        elif op == "i":
            t.insert(i, rng.choice(CHARS))
        else:
            del t[i]
    got, err = run("".join(t))
    if isinstance(err, str):
        other[err] = other.get(err, 0) + 1
expect(not other, f"damaged documents raised something other than ParseError: {other}")
done()
