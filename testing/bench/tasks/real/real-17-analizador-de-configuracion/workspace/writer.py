"""Writer for the confmini format (see SPEC.md). This side already works."""


def _string(s):
    out = ['"']
    for ch in s:
        if ch == '"':
            out.append('\\"')
        elif ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\r":
            out.append("\\r")
        elif ord(ch) < 0x20 or ord(ch) == 0x7F:
            out.append("\\u%04x" % ord(ch))
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def _value(v):
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, str):
        return _string(v)
    if isinstance(v, list):
        return "[" + ", ".join(_value(x) for x in v) + "]"
    raise TypeError(f"cannot write {type(v).__name__}")


def _section(data, path, lines):
    if path:
        lines.append("[" + ".".join(path) + "]")
    subs = []
    for key, value in data.items():
        if isinstance(value, dict):
            subs.append((key, value))
        else:
            lines.append(f"{key} = {_value(value)}")
    for key, value in subs:
        _section(value, path + (key,), lines)


def dumps(data):
    lines = []
    _section(data, (), lines)
    return "\n".join(lines) + "\n"
