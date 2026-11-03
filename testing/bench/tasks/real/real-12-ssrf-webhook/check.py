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

import ipaddress, socket

try:
    from fetcher import fetch
    from urlcheck import is_allowed
except Exception as e:
    print("FAIL: cannot import:", e)
    sys.exit(1)

TABLE = {
    "ok.example": ["93.184.216.34"],
    "rebind.example": ["93.184.216.34", "10.0.0.5"],
    "internal.example": ["10.1.2.3"],
    "localhost": ["127.0.0.1", "::1"],
    "metadata.google.internal": ["169.254.169.254"],
    "v6.example": ["2606:2800:220:1:248:1893:25c8:1946"],
    "v6local.example": ["::1"],
    "mapped.example": ["::ffff:127.0.0.1"],
    "ula.example": ["fd00::1"],
    "ll6.example": ["fe80::1"],
    "cgnat.example": ["100.64.0.1"],
    "multi.example": ["8.8.8.8", "::ffff:10.0.0.1"],
    "empty.example": [],
    "multicast.example": ["224.0.0.1"],
}


def resolver(host):
    # behaves like getaddrinfo: names from the table, numeric literals in any spelling
    if host in TABLE:
        return list(TABLE[host])
    try:
        return [socket.inet_ntoa(socket.inet_aton(host))]
    except OSError:
        pass
    try:
        return [str(ipaddress.ip_address(host))]
    except ValueError:
        raise OSError("unknown host " + host)


GOOD = [
    "http://ok.example/", "https://ok.example:8443/a?b=1", "https://v6.example/",
    "http://93.184.216.34/", "http://[2606:2800:220:1:248:1893:25c8:1946]/x",
]
BAD = [
    "ftp://ok.example/", "file:///etc/passwd", "gopher://ok.example/", "//ok.example/", "ok.example", "", "http:///etc",
    "http://127.0.0.1/", "http://localhost:8080/admin", "http://[::1]/", "http://2130706433/", "http://0x7f.1/",
    "http://0177.0.0.1/", "http://127.1/", "http://0/", "http://[::ffff:127.0.0.1]/", "http://169.254.169.254/latest/meta-data/",
    "http://metadata.google.internal/computeMetadata/v1/", "http://internal.example/", "http://rebind.example/",
    "http://mapped.example/", "http://v6local.example/", "http://ula.example/", "http://ll6.example/", "http://cgnat.example/",
    "http://multi.example/", "http://empty.example/", "http://multicast.example/", "http://unknown.example/",
    "http://ok.example@internal.example/", "http://ok.example@evil.example/", "http://user:pw@ok.example/", "http://@ok.example/",
    "http://ok.example:badport/", "http://10.0.0.1/", "http://192.168.1.1/", "http://172.16.5.4/", "http://100.64.0.1/",
    "http://[fd00::1]/", "http://[fe80::1]/",
]
for u in GOOD:
    try:
        expect(is_allowed(u, resolver) is True, f"public url refused: {u}")
    except Exception as e:
        expect(False, f"is_allowed raised on {u}: {type(e).__name__}")
for u in BAD:
    try:
        expect(is_allowed(u, resolver) is False, f"dangerous url allowed: {u!r}")
    except Exception as e:
        expect(False, f"is_allowed raised on {u!r}: {type(e).__name__} (must return False)")


def run(start, pages, **kw):
    calls = []

    def http_get(u):
        calls.append(u)
        if u not in pages:
            raise AssertionError("unexpected request " + u)
        return pages[u]

    try:
        return fetch(start, http_get, resolver, **kw), calls
    except ValueError:
        return "ValueError", calls
    except Exception as e:
        return type(e).__name__, calls


def R(to, key="Location"):
    return (302, {key: to}, b"")


res, calls = run("http://ok.example/a", {"http://ok.example/a": R("/b"), "http://ok.example/b": (200, {}, b"hello")})
expect(res == (200, b"hello") and calls == ["http://ok.example/a", "http://ok.example/b"], f"relative redirect: {res} {calls}")
res, _ = run("http://ok.example/a", {"http://ok.example/a": R("/b", "location"), "http://ok.example/b": (200, {}, b"x")})
expect(res == (200, b"x"), f"lowercase location header: {res}")
for evil in ["http://169.254.169.254/latest/meta-data/", "file:///etc/passwd", "http://ok.example@internal.example/", "http://internal.example/", "http://[::ffff:10.0.0.1]/", "//127.0.0.1/x", "http://2130706433/"]:
    res, calls = run("http://ok.example/a", {"http://ok.example/a": R(evil)})
    expect(res == "ValueError", f"redirect to {evil} was not refused: {res}")
    expect(len(calls) == 1, f"the blocked redirect target {evil} was requested: {calls}")
chain = {f"http://ok.example/{i}": R(f"/{i + 1}") for i in range(10)}
chain["http://ok.example/3"] = (200, {}, b"end")
res, _ = run("http://ok.example/0", chain)
expect(res == (200, b"end"), f"3 redirects must work: {res}")
chain["http://ok.example/4"] = (200, {}, b"end")
chain["http://ok.example/3"] = R("/4")
res, _ = run("http://ok.example/0", chain)
expect(res == "ValueError", f"4 redirects must fail with the default limit: {res}")
res, _ = run("http://ok.example/0", chain, max_redirects=4)
expect(res == (200, b"end"), f"max_redirects=4 must allow 4: {res}")
res, _ = run("http://ok.example/a", {"http://ok.example/a": R("/a")})
expect(res == "ValueError", f"redirect loop: {res}")
res, _ = run("http://ok.example/a", {"http://ok.example/a": (301, {}, b"")})
expect(res == "ValueError", f"redirect without Location: {res}")
res, calls = run("http://internal.example/", {})
expect(res == "ValueError" and calls == [], f"blocked start url: {res} {calls}")
res, _ = run("http://ok.example/a", {"http://ok.example/a": (404, {}, b"nope")})
expect(res == (404, b"nope"), f"non-redirect status is returned as is: {res}")
done()
