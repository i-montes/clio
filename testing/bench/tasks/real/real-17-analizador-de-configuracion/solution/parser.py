"""Parser for the confmini format (see SPEC.md)."""
import re

from textlines import MAX_DEPTH, split_lines


class ParseError(Exception):
    def __init__(self, line, message):
        super().__init__(f"line {line}: {message}")
        self.line = line
        self.message = message


_KEY = r"[A-Za-z_][A-Za-z0-9_-]*"
_HEADER = re.compile(rf"\[({_KEY}(?:\.{_KEY})*)\]")
_KEY_RE = re.compile(_KEY)
_TOKEN = re.compile(r"[A-Za-z0-9_.+-]*")
_INT = re.compile(r"-?(?:0|[1-9][0-9]*)")
_BARE = re.compile(r"[A-Za-z_][A-Za-z0-9_.-]*")
_ESCAPES = {'"': '"', "\\": "\\", "n": "\n", "t": "\t", "r": "\r"}


class _Cursor:
    def __init__(self, text, number):
        self.s, self.i, self.n = text, 0, number

    def fail(self, message):
        raise ParseError(self.n, message)

    def skip(self):
        while self.i < len(self.s) and self.s[self.i] in " \t":
            self.i += 1

    def at(self, ch):
        return self.i < len(self.s) and self.s[self.i] == ch

    def finish(self):
        self.skip()
        if self.i < len(self.s) and not self.at("#"):
            self.fail("unexpected text after the value")

    def string(self):
        self.i += 1
        out = []
        while True:
            if self.i >= len(self.s):
                self.fail("unterminated string")
            ch = self.s[self.i]
            self.i += 1
            if ch == '"':
                return "".join(out)
            if ch != "\\":
                out.append(ch)
                continue
            if self.i >= len(self.s):
                self.fail("unterminated string")
            esc = self.s[self.i]
            self.i += 1
            if esc in _ESCAPES:
                out.append(_ESCAPES[esc])
            elif esc == "u":
                digits = self.s[self.i : self.i + 4]
                if not re.fullmatch(r"[0-9A-Fa-f]{4}", digits) or 0xD800 <= int(digits, 16) <= 0xDFFF:
                    self.fail("bad \\u escape")
                out.append(chr(int(digits, 16)))
                self.i += 4
            else:
                self.fail(f"unknown escape \\{esc}")

    def array(self, depth):
        if depth > MAX_DEPTH:
            self.fail("lists nested too deeply")
        self.i += 1
        items = []
        self.skip()
        if self.at("]"):
            self.i += 1
            return items
        while True:
            items.append(self.value(depth))
            self.skip()
            if self.at(","):
                self.i += 1
                self.skip()
                if self.at("]"):
                    self.i += 1
                    return items
            elif self.at("]"):
                self.i += 1
                return items
            else:
                self.fail("expected ',' or ']'")

    def value(self, depth=0):
        self.skip()
        if self.i >= len(self.s):
            self.fail("missing value")
        if self.at('"'):
            return self.string()
        if self.at("["):
            return self.array(depth + 1)
        token = _TOKEN.match(self.s, self.i).group(0)
        self.i += len(token)
        if _INT.fullmatch(token):
            return int(token)
        if token in ("true", "false"):
            return token == "true"
        if _BARE.fullmatch(token):
            return token
        self.fail(f"invalid value {token!r}" if token else "missing value")


def parse(text):
    data, current, explicit = {}, None, set()
    current = data
    for number, raw in enumerate(split_lines(text), 1):
        cur = _Cursor(raw, number)
        cur.skip()
        if cur.i >= len(raw) or cur.at("#"):
            continue
        if cur.at("["):
            m = _HEADER.match(raw, cur.i)
            if not m:
                cur.fail("bad section header")
            cur.i = m.end()
            cur.finish()
            path = tuple(m.group(1).split("."))
            if path in explicit:
                cur.fail("section defined twice")
            explicit.add(path)
            current = data
            for part in path:
                if part not in current:
                    current[part] = {}
                elif not isinstance(current[part], dict):
                    cur.fail(f"{part!r} is already a value")
                current = current[part]
            continue
        m = _KEY_RE.match(raw, cur.i)
        if not m:
            cur.fail("expected a key")
        cur.i = m.end()
        cur.skip()
        if not cur.at("="):
            cur.fail("expected '='")
        cur.i += 1
        value = cur.value()
        cur.finish()
        if m.group(0) in current:
            cur.fail(f"duplicate key {m.group(0)!r}")
        current[m.group(0)] = value
    return data
