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

import re
from html.parser import HTMLParser

try:
    from pages import render_profile_page, render_search_box
    from render import render_card
except Exception as e:
    print("FAIL: cannot import:", e)
    sys.exit(1)

ALLOWED_TAGS = {"html", "head", "title", "body", "div", "h2", "p", "a", "ul", "li", "form", "input"}


class P(HTMLParser):
    def __init__(self):
        super().__init__()
        self.tags, self.stack, self.text = [], [], {}

    def handle_starttag(self, tag, attrs):
        self.tags.append((tag, dict(attrs)))
        self.stack.append(tag)

    def handle_endtag(self, tag):
        if self.stack and self.stack[-1] == tag:
            self.stack.pop()

    def handle_data(self, data):
        if self.stack:
            self.text.setdefault(self.stack[-1], []).append(data)


def parse(html):
    p = P()
    p.feed(html)
    p.close()
    return p


def scheme_ok(url):
    probe = "".join(c for c in url if ord(c) > 0x20 and ord(c) != 0x7F).lower()
    m = re.match(r"^[a-z][a-z0-9+.\-]*:", probe)
    return m is None or m.group(0)[:-1] in ("http", "https", "mailto")


def audit(html, label):
    p = parse(html)
    for tag, attrs in p.tags:
        expect(tag in ALLOWED_TAGS, f"{label}: injected tag <{tag}>")
        for k, v in attrs.items():
            expect(not k.startswith("on"), f"{label}: injected attribute {k}")
            expect(k in {"class", "data-name", "href", "action", "type", "name", "value"}, f"{label}: unexpected attribute {k}")
        if tag == "a":
            expect(scheme_ok(attrs.get("href") or ""), f"{label}: dangerous href {attrs.get('href')!r}")
    return p


NASTY = [
    '<script>alert(1)</script>',
    '"><img src=x onerror=alert(1)>',
    "' onmouseover='alert(1)",
    '" onfocus="alert(1)" autofocus="',
    "Tom & Jerry <3 &amp; &lt;b&gt;",
    "plain ñandú café",
    "</div><div class=\"card\">fake",
]
BAD_URLS = [
    "javascript:alert(1)",
    "JaVaScRiPt:alert(1)",
    "  javascript:alert(1)",
    "java\tscript:alert(1)",
    "java\nscript:alert(1)",
    "\x01javascript:alert(1)",
    "data:text/html,<script>alert(1)</script>",
    "vbscript:msgbox(1)",
    "JAVASCRIPT&#58;alert(1)",
    "javascript:alert('x')\"onmouseover=\"alert(1)",
]
GOOD_URLS = ["https://example.org/a?b=1&c=2", "http://example.org", "/about", "about.html", "mailto:ana@example.org", "#top", "?q=1"]

for i, text in enumerate(NASTY):
    html = render_card(text, text, "https://example.org/", [text, "ok"])
    p = audit(html, f"card text #{i}")
    divs = [a for t, a in p.tags if t == "div"]
    expect(len(divs) == 1 and divs[0].get("data-name") == text, f"card #{i}: data-name does not round-trip")
    expect("".join(p.text.get("h2", [])) == text, f"card #{i}: name does not round-trip as text")
    expect("".join(p.text.get("p", [])) == text, f"card #{i}: bio does not round-trip as text")
    expect([x for x in p.text.get("li", [])] == [text, "ok"], f"card #{i}: tags do not round-trip")
    html = render_search_box(text)
    p = audit(html, f"search #{i}")
    inputs = [a for t, a in p.tags if t == "input"]
    expect(len(inputs) == 1 and inputs[0].get("value") == text, f"search #{i}: value does not round-trip")
    page = render_profile_page(text, [{"name": text, "bio": text, "website": "/x", "tags": [text]}])
    p = audit(page, f"page #{i}")
    expect("".join(p.text.get("title", [])) == text, f"page #{i}: title does not round-trip")
    expect(sum(1 for t, a in p.tags if t == "div") == 1, f"page #{i}: card count changed")
    expect("".join(p.text.get("h2", [])) == text, f"page #{i}: card was escaped twice or broken")

for url in BAD_URLS:
    html = render_card("n", "b", url, [])
    audit(html, f"url {url!r}")
    page = render_profile_page("t", [{"name": "n", "bio": "b", "website": url}])
    audit(page, f"page url {url!r}")

for url in GOOD_URLS:
    p = parse(render_card("n", "b", url, []))
    hrefs = [a.get("href") for t, a in p.tags if t == "a"]
    expect(hrefs == [url], f"legitimate url {url!r} was changed to {hrefs}")
expect("b=1&amp;c=2" in render_card("n", "b", GOOD_URLS[0], []), "the & of a legitimate url is not escaped in the HTML")
done()
